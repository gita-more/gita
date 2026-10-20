//! Pipeline execution: clone, parse YAML, run jobs, collect logs.

use crate::config::RunnerConfig;
use crate::registry::JobRegistry;
use crate::workspace::{clone_url, job_work_dir, run_work_dir};
use anyhow::Context;
use chrono::Utc;
use gita_db::{Db, PipelineRunRow};
use gita_pipeline::{expand, parse_yaml};
use std::process::Stdio;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::{interval, Duration};

/// Cancellation poll interval — how often the executor checks the
/// pipeline_runs.status while a job is running.
const CANCEL_POLL: Duration = Duration::from_secs(1);

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
    registry: &JobRegistry,
    run: &PipelineRunRow,
) -> anyhow::Result<()> {
    let work_dir = run_work_dir(config, run.id);
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
    // If the repo is already present (e.g. a prior run left it, or an
    // integration test pre-cloned it), skip the network clone.
    if !repo_dir.join(".git").exists() {
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
    let mut canceled = false;
    for plan in &plans {
        // Skip the remaining stages if the run was canceled.
        if let Ok(Some(fresh)) = db.get_pipeline_run(run.id).await {
            if fresh.status == "canceled" {
                canceled = true;
                break;
            }
        }
        let ok = run_job(db, config, registry, run, &plan.job).await?;
        if !ok {
            any_failed = true;
            // If the job was canceled (vs. failed for some other
            // reason), don't keep starting more jobs and don't
            // overwrite the canceled run status.
            if let Ok(Some(fresh)) = db.get_pipeline_run(run.id).await {
                if fresh.status == "canceled" {
                    canceled = true;
                }
            }
            break;
        }
    }

    if canceled {
        // The run row is already in 'canceled'; jobs are marked
        // 'canceled' or 'failed' by run_job. Don't overwrite.
        return Ok(());
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
    registry: &JobRegistry,
    run: &PipelineRunRow,
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

    let child = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .current_dir(&repo_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()
        .context("spawn sh")?;

    // Register the child so a cancel-watcher (spawned below) can find
    // it via JobRegistry::kill_run.
    registry.register(run.id, &job.name, child).await;

    // Read stdout/stderr line-buffered and insert into logs.
    let seq = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));

    let mut child = registry
        .take(run.id, &job.name)
        .await
        .ok_or_else(|| anyhow::anyhow!("child disappeared from registry"))?;

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

    // Race the child against a cancel poll. The cancel poll runs every
    // CANCEL_POLL on a separate task; if pipeline_runs.status becomes
    // 'canceled' the watcher calls registry.kill_run, which start_kill()s
    // the registered child (here, `child` is a fresh handle we just took
    // out, so it isn't in the registry anymore — the watcher needs the
    // same kill semantics on the local handle).
    //
    // To avoid the `&mut Child` conflict between wait() and start_kill()
    // (they can't run concurrently), we keep the child in the registry
    // during execution and use an mpsc channel to ask the main loop to
    // kill it.
    let (kill_tx, mut kill_rx) = tokio::sync::mpsc::channel::<()>(1);
    // Clone for the watcher so we can drop our copy and detach the
    // background task once we've awaited the child.
    let watcher_kill_tx = kill_tx.clone();
    let registry_for_watcher = registry.clone();
    let watcher_db = db.clone();
    let watcher_run_id = run.id;
    let _watcher = tokio::spawn(async move {
        let mut ticker = interval(CANCEL_POLL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let Ok(Some(fresh)) = watcher_db.get_pipeline_run(watcher_run_id).await
            else {
                break;
            };
            if fresh.status == "canceled" {
                let _ = registry_for_watcher.kill_run(watcher_run_id).await;
                let _ = watcher_kill_tx.send(()).await;
                break;
            }
            if !matches!(fresh.status.as_str(), "running" | "queued") {
                break;
            }
        }
    });

    // Drop our own registry reference: this task doesn't need to kill
    // the child (the watcher owns that). The watcher keeps the registry
    // alive.
    drop(registry.clone());

    let status = tokio::select! {
        s = child.wait() => s.context("wait for sh")?,
        _ = kill_rx.recv() => {
            // Cancel observed — start_kill and reap.
            let _ = child.start_kill();
            // The child is a process-group leader (process_group(0)), so
            // kill the whole group (-pid) to make sure grandchildren (e.g.
            // `sleep 30` spawned by a non-exec-optimizing shell) do not
            // keep the stdout/stderr pipes open after the shell exits.
            // Note the `--`: without it, `kill -TERM -<pid>` interprets
            // `-<pid>` as an invalid signal number and hangs.
            #[cfg(unix)]
            {
                if let Some(pid) = child.id() {
                    let _ = Command::new("kill")
                        .args(["-TERM", "--", &format!("-{pid}")])
                        .status()
                        .await;
                }
            }
            child.wait().await.context("wait after kill")?
        }
    };
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), stdout_task).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), stderr_task).await;
    // Drop our kill channel so the watcher's send() returns Err and it
    // doesn't keep polling uselessly after the run already finished.
    drop(kill_tx);

    // Determine outcome. SIGTERM (signal 15) is what tokio uses for
    // start_kill on Unix; treat that as canceled, anything else
    // non-zero as failed.
    let exit_code = status.code();
    // On Unix, start_kill sends SIGTERM (signal 15). Tokio surfaces a
    // signal-killed exit as `code() == None` + `signal() == Some(15)`,
    // so we have to check both shapes. On non-Unix we fall back to the
    // exit-code check (143 = 128 + 15) since ExitStatusExt isn't
    // available.
    #[cfg(unix)]
    let was_canceled = matches!(exit_code, Some(143))
        || matches!(status.signal(), Some(15) | Some(9));
    #[cfg(not(unix))]
    let was_canceled = matches!(exit_code, Some(143));
    let ok = status.success();
    let final_status = if was_canceled {
        "canceled"
    } else if ok {
        "success"
    } else {
        "failed"
    };

    db.update_job_status(job_id, final_status, exit_code).await?;

    // Clean up the job workspace to avoid accumulating dirs.
    tokio::fs::remove_dir_all(&job_dir).await.ok();

    Ok(ok)
}

/// Resolve (owner, repo) for a run from the repositories table.
async fn get_repo_owner_name(
    db: &Db,
    run: &PipelineRunRow,
) -> anyhow::Result<Option<(String, String)>> {
    // The repositories table has owner_name + name; fetch by repo_id.
    let repo = db.get_repository(run.repo_id).await?;
    Ok(repo.map(|r| (r.owner_name, r.name)))
}