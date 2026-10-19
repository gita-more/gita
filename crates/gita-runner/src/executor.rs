//! Pipeline execution: clone, parse YAML, run jobs, collect logs.

use crate::claim::ClaimedRun;
use crate::config::RunnerConfig;
use crate::workspace::{clone_url, job_work_dir, run_work_dir};
use anyhow::Context;
use chrono::Utc;
use gita_db::Db;
use gita_pipeline::{expand, parse_yaml};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Execute one pipeline run end-to-end.
///
/// 1. Clone the repo at `head_sha` into the run workspace.
/// 2. Read `.gita-ci.yml` from the clone.
/// 3. Parse + expand it into JobPlans.
/// 4. Run each job in order, writing log lines as they come.
/// 5. Mark the run terminal (`success` or `failed`) and publish the
///    corresponding event.
pub async fn execute_pipeline(
    db: &Db,
    config: &RunnerConfig,
    run: &ClaimedRun,
) -> anyhow::Result<()> {
    let work_dir = run_work_dir(config, run.id);
    if work_dir.exists() {
        tokio::fs::remove_dir_all(&work_dir).await.ok();
    }
    tokio::fs::create_dir_all(&work_dir)
        .await
        .with_context(|| format!("create work dir {}", work_dir.display()))?;

    // Clone the repo (owner is the repo's owner from repo_id; the run
    // row stores ref + head_sha but not owner/name — resolve them).
    // We read owner/repo from the DB via repo_id.
    let (owner, repo_name) = match get_repo_owner_name(db, run).await? {
        Some(v) => v,
        None => {
            // Repo was deleted while the run was queued — fail the run.
            db.update_pipeline_run_status(run.id, "failed").await?;
            return Ok(());
        }
    };

    let repo_dir = work_dir.join("repo");
    let url = clone_url(config, &owner, &repo_name);
    let clone_status = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--branch")
        .arg(&run.ref_name)
        .arg(&url)
        .arg(&repo_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .await
        .context("git clone")?;

    if !clone_status.success() {
        // Branch may not exist anymore or ref is deleted; mark run failed.
        db.update_pipeline_run_status(run.id, "failed").await?;
        return Ok(());
    }

    // Read .gita-ci.yml at the clone.
    let yaml_path = repo_dir.join(".gita-ci.yml");
    let yaml = match tokio::fs::read_to_string(&yaml_path).await {
        Ok(s) => s,
        Err(_) => {
            db.update_pipeline_run_status(run.id, "failed").await?;
            return Ok(());
        }
    };

    let spec = match parse_yaml(&yaml) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("parse .gita-ci.yml failed: {e}");
            db.update_pipeline_run_status(run.id, "failed").await?;
            return Ok(());
        }
    };

    let plans = match expand(&spec) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("expand pipeline failed: {e}");
            db.update_pipeline_run_status(run.id, "failed").await?;
            return Ok(());
        }
    };

    let mut any_failed = false;
    for plan in &plans {
        let ok = run_job(db, config, run, &plan.job).await?;
        if !ok {
            any_failed = true;
            break;
        }
    }

    db.update_pipeline_run_status(run.id, if any_failed { "failed" } else { "success" })
        .await?;
    Ok(())
}

/// Run a single job: insert row, publish event, exec `sh -c`, collect logs.
/// Returns `true` if the job succeeded.
async fn run_job(
    db: &Db,
    config: &RunnerConfig,
    run: &ClaimedRun,
    job: &gita_pipeline::ParsedJob,
) -> anyhow::Result<bool> {
    let job_id = db
        .insert_job(
            run.id,
            &job.name,
            &job.stage,
            &job.needs,
            &job.script,
        )
        .await?;

    db.update_job_status(job_id, "running", None).await?;

    // Create the job workspace (a fresh clone in a per-job dir).
    let job_dir = job_work_dir(config, run.id, &job.name);
    if job_dir.exists() {
        tokio::fs::remove_dir_all(&job_dir).await.ok();
    }
    tokio::fs::create_dir_all(&job_dir).await?;

    // For Phase 1 we share the repo clone with the job workspace via a
    // symlink so the job sees the checkout without a second clone.
    // Simpler: we run the script with cwd = the repo clone. Jobs that
    // need isolation can copy. To keep logs tied to the job we keep the
    // same repo dir (already cloned).
    let repo_dir = run_work_dir(config, run.id).join("repo");
    let _ = &repo_dir;

    // Join the script into a single shell command.
    let joined_script = job.script.join(" && ");
    let script = format!("set -e\n{joined_script}");

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .current_dir(&repo_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .context("spawn sh")?;

    // Read stdout/stderr line-buffered and insert into logs.
    let seq = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));

    let mut stdout_reader = BufReader::new(child.stdout.take().unwrap()).lines();
    let stderr_reader = child.stderr.take().unwrap();
    let mut stderr_reader = BufReader::new(stderr_reader).lines();

    let db_stdout = db.clone();
    let job_id_stdout = job_id;
    let seq_stdout = seq.clone();
    let stdout_task = tokio::spawn(async move {
        while let Ok(Some(line)) = stdout_reader.next_line().await {
            let s = seq_stdout.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = db_stdout
                .insert_log(
                    job_id_stdout,
                    s,
                    "stdout",
                    &line,
                    &Utc::now().to_rfc3339(),
                )
                .await;
        }
    });

    let db_stderr = db.clone();
    let job_id_stderr = job_id;
    let seq_stderr = seq.clone();
    let stderr_task = tokio::spawn(async move {
        while let Ok(Some(line)) = stderr_reader.next_line().await {
            let s = seq_stderr.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = db_stderr
                .insert_log(
                    job_id_stderr,
                    s,
                    "stderr",
                    &line,
                    &Utc::now().to_rfc3339(),
                )
                .await;
        }
    });

    let status = child.wait().await.context("wait for sh")?;
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    let exit_code = status.code();
    let ok = status.success();

    db.update_job_status(job_id, if ok { "success" } else { "failed" }, exit_code)
        .await?;

    // Clean up the job workspace to avoid accumulating dirs.
    tokio::fs::remove_dir_all(&job_dir).await.ok();

    Ok(ok)
}

/// Resolve (owner, repo) for a run from the repositories table.
async fn get_repo_owner_name(
    db: &Db,
    run: &ClaimedRun,
) -> anyhow::Result<Option<(String, String)>> {
    // The repositories table has owner_name + name; fetch by repo_id.
    let repo = db.get_repository(run.repo_id).await?;
    Ok(repo.map(|r| (r.owner_name, r.name)))
}