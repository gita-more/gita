//! Pipeline execution: clone, parse YAML, run jobs, collect logs.

use crate::config::RunnerConfig;
use crate::registry::JobRegistry;
use crate::workspace::{clone_url, job_work_dir, run_work_dir};
use anyhow::Context;
use chrono::Utc;
use gita_db::{Db, PipelineRunRow};
use gita_pipeline::{expand, parse_yaml};
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::process::Stdio;
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
        // The run stores the full ref (`refs/heads/main`), but `git clone
        // --branch` wants a short branch name — it does not strip the
        // prefix, and `refs/heads/main` fails with "Remote branch not
        // found". Clone by short name, then check out the exact commit the
        // run was created for, so a run tests what it says it tests even
        // if the branch has moved on since.
        let short_branch = run
            .ref_name
            .strip_prefix("refs/heads/")
            .unwrap_or(&run.ref_name);
        // Pipe the child's output rather than inheriting it: a clone that
        // fails should have its error surfaced in the log, not swallowed
        // into the runner's own stdout.
        let clone_output = Command::new("git")
            .arg("clone")
            .arg("--depth")
            .arg("1")
            .arg("--branch")
            .arg(&short_branch)
            .arg(&url)
            .arg(&repo_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("git clone")?;

        if !clone_output.status.success() {
            // Branch may not exist anymore or ref is deleted; mark run failed.
            db.update_pipeline_run_status(run.id, "failed").await?;
            return Ok(());
        }

        // Check out the exact commit the run was created for. The clone
        // fetched the branch tip, which may have moved on since the run was
        // queued — the run would then execute a different commit than the
        // one it reports. A shallow clone may not contain the SHA at all,
        // so fetch it explicitly first.
        if !run.head_sha.is_empty() {
            // Fetch the exact commit. A shallow clone only contains the
            // branch tip, so the commit the run was created for has to be
            // fetched explicitly — and the fetch also deepens the history
            // enough for `checkout --detach` to succeed (checking out a
            // commit whose parent is missing fails even though the commit
            // object itself is present).
            //
            // A non-shallow clone (e.g. from a `file://` remote, where
            // git ignores `--depth`) already has the commit; the fetch
            // then fails harmlessly, which is why the result is not
            // treated as fatal on its own.
            let fetch_status = Command::new("git")
                .arg("-C")
                .arg(&repo_dir)
                .arg("fetch")
                .arg("--depth")
                .arg("1")
                .arg("origin")
                .arg(&run.head_sha)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .status()
                .await
                .context("git fetch")?;
            let have_commit = Command::new("git")
                .arg("-C")
                .arg(&repo_dir)
                .arg("cat-file")
                .arg("-e")
                .arg(&format!("{}^{{commit}}", run.head_sha))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false);
            if fetch_status.success() || have_commit {
                let co = Command::new("git")
                    .arg("-C")
                    .arg(&repo_dir)
                    .arg("checkout")
                    .arg("--detach")
                    .arg(&run.head_sha)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output()
                    .await
                    .context("git checkout")?;
                let checkout_status = co.status;
                if !checkout_status.success() {}
                if !checkout_status.success() {
                    // The run records `head_sha`, but the working tree is
                    // not at that commit. Running anyway would make the
                    // run report one commit while executing another.
                    tracing::warn!(
                        "checkout of {} failed; failing the run rather than                          executing a different commit",
                        run.head_sha
                    );
                    db.update_pipeline_run_status(run.id, "failed").await?;
                    return Ok(());
                }
            } else {
                // The commit is no longer reachable (force-pushed away).
                // Falling back to the branch tip would make the run report
                // `head_sha` while executing something else — fail the run
                // instead so the mismatch is visible.
                tracing::warn!(
                    "fetch of {} failed; failing the run rather than                      executing the branch tip instead",
                    run.head_sha
                );
                db.update_pipeline_run_status(run.id, "failed").await?;
                return Ok(());
            }
        }
    }

    // Read .gita-ci.yml at the clone.
    let yaml_path = repo_dir.join(".gita-ci.yml");
    let yaml = match tokio::fs::read_to_string(&yaml_path).await {
        Ok(s) => s,
        Err(e) => {
            let listing = std::fs::read_dir(repo_dir)
                .map(|d| {
                    d.filter_map(|e| e.ok())
                        .map(|e| e.file_name().to_string_lossy().to_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            tracing::warn!("read .gita-ci.yml failed: {e}; repo dir contains: {listing:?}");
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

    let final_run_status = if any_failed { "failed" } else { "success" };
    db.update_pipeline_run_status(run.id, final_run_status)
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
        .insert_job(run.id, &job.name, &job.stage, &job.needs, &job.script)
        .await?;

    db.update_job_status(job_id, "running", None).await?;

    // Create the job workspace (a fresh clone in a per-job dir).
    let job_dir = job_work_dir(config, run.id, &job.name);
    if job_dir.exists() {
        tokio::fs::remove_dir_all(&job_dir).await.ok();
    }
    tokio::fs::create_dir_all(&job_dir).await?;

    // The repository's owner/name, for the job's environment. The run row
    // carries only `repo_id`.
    let repo_label = match get_repo_owner_name(db, run).await? {
        Some((owner, repo_name)) => format!("{owner}/{repo_name}"),
        None => String::new(),
    };

    // Each job gets its own working directory, created as a git worktree
    // of the run's clone. Previously every job ran with the *same* cwd
    // (the run's clone), so a job could see — and overwrite — the files
    // another job in the same run had produced.
    let repo_dir = run_work_dir(config, run.id).join("repo");
    let job_cwd = match make_job_worktree(&repo_dir, &job_dir, &run.head_sha).await {
        Some(dir) => dir,
        // If the worktree cannot be created, fall back to the shared
        // clone so the job still runs.
        None => repo_dir.clone(),
    };

    // Join the script into a single shell command.
    let joined_script = job.script.join(" && ");
    let script = format!("set -e\n{joined_script}");

    // The job's environment is built from scratch rather than inherited.
    //
    // The runner process holds `GITA_DATABASE_URL` (with the database
    // password) and `GITA_RUNNER_TOKEN` (a personal access token that can
    // push to every repository its owner owns). A job script is arbitrary
    // shell from a pushed file; if it inherited the runner's environment,
    // `env` alone would hand it both credentials. Start from an empty
    // environment and add back only what a build needs.
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(&script)
        .current_dir(&job_cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .process_group(0)
        .env_clear();
    // A minimal PATH so `/usr/bin/env` and the usual toolchain still work.
    cmd.env(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| {
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string()
        }),
    );
    // HOME points at the job's own workspace, not the runner user's home.
    cmd.env("HOME", &job_dir);
    // Tell the script which repository it is running against, without
    // exposing any credential.
    cmd.env("GITA_REPOSITORY", &repo_label);
    cmd.env("GITA_COMMIT_SHA", &run.head_sha);
    cmd.env("GITA_REF", &run.ref_name);
    cmd.env("GITA_JOB_NAME", &job.name);
    cmd.env("GITA_RUN_ID", run.id.to_string());
    // Keep the runner's own environment out of the job.
    cmd.env_remove("GITA_DATABASE_URL");
    cmd.env_remove("GITA_RUNNER_TOKEN");
    cmd.env_remove("DATABASE_URL");
    cmd.env_remove("GITA_SERVER_URL");
    cmd.env_remove("GITA_SERVER_DB_URL");

    let child = cmd.spawn().context("spawn sh")?;

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
                .insert_log(job_id_stdout, s, "stdout", &line, &Utc::now().to_rfc3339())
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
                .insert_log(job_id_stderr, s, "stderr", &line, &Utc::now().to_rfc3339())
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
            let Ok(Some(fresh)) = watcher_db.get_pipeline_run(watcher_run_id).await else {
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

    // A job script is arbitrary shell; a wall-clock limit keeps one
    // runaway job from pinning the runner forever.
    let timeout = config.job_timeout();
    // `Box::pin` makes the optional timeout a concrete future, so it can
    // take part in the `select!` below.
    let sleep_until_timeout: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        match timeout {
            Some(d) => Box::pin(tokio::time::sleep(d)),
            None => Box::pin(std::future::pending()),
        };
    let mut timed_out = false;

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
        _ = sleep_until_timeout => {
            timed_out = true;
            // The job exceeded its wall-clock limit. Kill it the same way
            // a cancel does, so the whole process group goes away.
            tracing::warn!(
                job = %job.name,
                timeout_secs = ?timeout,
                "job exceeded its timeout; killing it"
            );
            let _ = child.start_kill();
            #[cfg(unix)]
            {
                if let Some(pid) = child.id() {
                    let _ = Command::new("kill")
                        .args(["-TERM", "--", &format!("-{pid}")])
                        .status()
                        .await;
                }
            }
            child.wait().await.context("wait after timeout kill")?
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
    let was_signaled =
        matches!(exit_code, Some(143)) || matches!(status.signal(), Some(15) | Some(9));
    #[cfg(not(unix))]
    let was_signaled = matches!(exit_code, Some(143));
    // A job killed by the timeout is a failure, not a cancellation: the
    // run was not cancelled by anyone, the script simply took too long.
    let was_canceled = was_signaled && !timed_out;
    let ok = status.success();
    let final_status = if was_canceled {
        "canceled"
    } else if ok {
        "success"
    } else {
        "failed"
    };

    db.update_job_status(job_id, final_status, exit_code)
        .await?;

    // Clean up the job workspace to avoid accumulating dirs. A git
    // worktree has to be removed through git so the clone's worktree
    // bookkeeping stays consistent.
    if job_cwd == job_dir {
        let _ = Command::new("git")
            .arg("-C")
            .arg(&repo_dir)
            .arg("worktree")
            .arg("remove")
            .arg("--force")
            .arg(&job_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;
    }
    tokio::fs::remove_dir_all(&job_dir).await.ok();

    Ok(ok)
}

/// Create a per-job working directory as a git worktree of the run's
/// clone.
///
/// A worktree is cheap (no second clone) and gives the job its own
/// checkout, so jobs in the same run cannot see or clobber each other's
/// files. Returns `None` if the worktree could not be created, in which
/// case the caller falls back to the shared clone.
async fn make_job_worktree(
    repo_dir: &std::path::Path,
    job_dir: &std::path::Path,
    head_sha: &str,
) -> Option<std::path::PathBuf> {
    if !repo_dir.join(".git").exists() {
        return None;
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .arg("worktree")
        .arg("add")
        .arg("--detach")
        .arg(job_dir)
        .arg(head_sha)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        tracing::warn!(
            "git worktree add failed for {}: {}",
            job_dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    Some(job_dir.to_path_buf())
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
