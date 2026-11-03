//! Pipeline execution: clone, parse YAML, run jobs, collect logs.

use crate::client::{LogLine, RunnerClient};
use crate::clone::{prepare_run_repo, resolve_repo_owner_name, CloneOutcome};
use crate::config::RunnerConfig;
use crate::registry::JobRegistry;
use crate::workspace::{job_work_dir, run_work_dir};
use anyhow::Context;
use chrono::Utc;
use gita_db::PipelineRunRow;
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
    client: &RunnerClient,
    config: &RunnerConfig,
    registry: &JobRegistry,
    run: &PipelineRunRow,
) -> anyhow::Result<()> {
    let work_dir = run_work_dir(config, run.id);
    tokio::fs::create_dir_all(&work_dir)
        .await
        .with_context(|| format!("create work dir {}", work_dir.display()))?;

    // Prepare the checkout. The clone/checkout logic is shared with the
    // workflow executor so both paths check out exactly the commit the
    // run names.
    let repo_dir = match prepare_run_repo(client, config, run).await? {
        CloneOutcome::Ready(dir) => dir,
        CloneOutcome::Failed(reason) => {
            tracing::warn!("pipeline checkout failed: {reason}");
            client.set_run_status(run.id, "failed").await?;
            return Ok(());
        }
    };

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
            client.set_run_status(run.id, "failed").await?;
            return Ok(());
        }
    };

    let spec = match parse_yaml(&yaml) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("parse .gita-ci.yml failed: {e}");
            client.set_run_status(run.id, "failed").await?;
            return Ok(());
        }
    };

    let plans = match expand(&spec) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("expand pipeline failed: {e}");
            client.set_run_status(run.id, "failed").await?;
            return Ok(());
        }
    };

    let mut any_failed = false;
    let mut canceled = false;
    for plan in &plans {
        // Skip the remaining stages if the run was canceled.
        if let Ok(fresh) = client.get_run(run.id).await {
            if fresh.status == "canceled" {
                canceled = true;
                break;
            }
        }
        let ok = run_job(client, config, registry, run, &plan.job).await?;
        if !ok {
            any_failed = true;
            // If the job was canceled (vs. failed for some other
            // reason), don't keep starting more jobs and don't
            // overwrite the canceled run status.
            if let Ok(fresh) = client.get_run(run.id).await {
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
    client.set_run_status(run.id, final_run_status).await?;
    Ok(())
}

/// Run a single job: insert row, publish event, exec `sh -c`, collect logs.
/// Returns `true` if the job succeeded.
async fn run_job(
    client: &RunnerClient,
    config: &RunnerConfig,
    registry: &JobRegistry,
    run: &PipelineRunRow,
    job: &gita_pipeline::ParsedJob,
) -> anyhow::Result<bool> {
    let job_id = client
        .create_job(run.id, &job.name, &job.stage, &job.needs, &job.script)
        .await?;

    // A `vm:` job runs inside a cluster virtual machine: the runner only
    // provisions the VM and watches; the agent inside the VM reports
    // running/success/failed through the agent endpoints. The runner
    // must not mark the job running here — the agent owns those
    // transitions while a VM exists.
    if let Some(vm) = &job.vm {
        let script = job.script.join("\n");
        let services_json = serde_json::to_string(&job.services)
            .context("serialize the job's services")?;
        let outcome = crate::vm_exec::run_job_in_vm_with_services(
            client,
            config,
            run,
            job_id,
            vm,
            &script,
            &services_json,
        )
        .await?;
        return Ok(match outcome {
            crate::vm_exec::VmJobOutcome::Success => {
                // The agent already finished the row; the caller expects
                // the run-level status to move on.
                true
            }
            crate::vm_exec::VmJobOutcome::Canceled => {
                // The row is already 'canceled'; don't touch it.
                false
            }
            // Infrastructure failures and timeouts are job failures —
            // but only if the agent didn't settle the row first
            // (update_job_status overwrites unconditionally, and the
            // agent may legitimately have written 'failed' already; a
            // repeated 'failed' is the same outcome either way).
            crate::vm_exec::VmJobOutcome::Failed
            | crate::vm_exec::VmJobOutcome::TimedOut
            | crate::vm_exec::VmJobOutcome::InfrastructureError => {
                client.set_job_status(job_id, "failed", None).await?;
                false
            }
        });
    }

    client.set_job_status(job_id, "running", None).await?;

    // Create the job workspace (a fresh clone in a per-job dir).
    let job_dir = job_work_dir(config, run.id, &job.name);
    if job_dir.exists() {
        tokio::fs::remove_dir_all(&job_dir).await.ok();
    }
    tokio::fs::create_dir_all(&job_dir).await?;

    // The repository's owner/name, for the job's environment. The run row
    // carries only `repo_id`.
    let repo_label = match resolve_repo_owner_name(client, run).await? {
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
    // The runner process holds `GITA_RUNNER_TOKEN` (a personal access
    // token that can push to every repository its owner owns). A job
    // script is arbitrary shell from a pushed file; if it inherited the
    // runner's environment, `env` alone would hand it the credential.
    // Start from an empty environment and add back only what a build
    // needs.
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
    // Keep the runner's own credentials out of the job.
    cmd.env_remove("GITA_RUNNER_TOKEN");
    cmd.env_remove("GITA_SERVER_URL");

    // Start the job's service containers before anything runs, so the
    // first step can connect. Torn down at the end whatever the outcome.
    let services_sink = client.clone();
    let services_job_id = job_id;
    let services_log = move |line: &str| {
        let client = services_sink.clone();
        let line = line.to_string();
        tokio::spawn(async move {
            let _ = client
                .append_logs(
                    services_job_id,
                    &[crate::client::LogLine {
                        seq: 0,
                        stream: "system".to_string(),
                        line,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    }],
                )
                .await;
        });
    };
    let mut services =
        match gita_ci::services::Services::start(job_id, &job.services, &services_log).await {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(job = %job.name, "services failed to start: {e:#}");
                client.set_job_status(job_id, "failed", None).await?;
                return Ok(false);
            }
        };

    let child = cmd.spawn().context("spawn sh")?;

    // Register the child so a cancel-watcher (spawned below) can find
    // it via JobRegistry::kill_run.
    registry.register(run.id, &job.name, child).await;

    // Read stdout/stderr line-buffered and send them to the server.
    //
    // The lines are batched: one HTTP request per log line would make a
    // chatty build spend most of its time on round trips. A batch goes
    // out when it reaches LOG_BATCH_LINES or when the flush interval
    // elapses, whichever comes first — so the last lines of a job still
    // show up promptly.
    let seq = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));

    let mut child = registry
        .take(run.id, &job.name)
        .await
        .ok_or_else(|| anyhow::anyhow!("child disappeared from registry"))?;

    let stdout_reader = BufReader::new(child.stdout.take().unwrap()).lines();
    let stderr_reader = child.stderr.take().unwrap();
    let stderr_reader = BufReader::new(stderr_reader).lines();

    let client_stdout = client.clone();
    let job_id_stdout = job_id;
    let seq_stdout = seq.clone();
    let stdout_task = tokio::spawn(async move {
        stream_logs(
            client_stdout,
            job_id_stdout,
            seq_stdout,
            "stdout",
            stdout_reader,
        )
        .await;
    });

    let client_stderr = client.clone();
    let job_id_stderr = job_id;
    let seq_stderr = seq.clone();
    let stderr_task = tokio::spawn(async move {
        stream_logs(
            client_stderr,
            job_id_stderr,
            seq_stderr,
            "stderr",
            stderr_reader,
        )
        .await;
    });

    // Race the child against a cancel poll. The cancel poll runs every
    // CANCEL_POLL on a separate task; if the run's status becomes
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
    let watcher_client = client.clone();
    let watcher_run_id = run.id;
    let _watcher = tokio::spawn(async move {
        let mut ticker = interval(CANCEL_POLL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let Ok(fresh) = watcher_client.get_run(watcher_run_id).await else {
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

    client
        .set_job_status(job_id, final_status, exit_code)
        .await?;

    // Tear the service containers down now that the job has settled.
    if let Some(mut services) = services.take() {
        services.stop().await;
    }

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

/// How many log lines go out in one request.
const LOG_BATCH_LINES: usize = 32;

/// How long a partial batch waits before it is sent anyway.
const LOG_FLUSH_INTERVAL: Duration = Duration::from_millis(500);

/// Read a stream of log lines and send them to the server in batches.
///
/// The batch is flushed when it is full or when the flush interval
/// elapses — the latter matters for a job whose last line arrives long
/// before the job ends: the log has to be visible while the job is still
/// running.
async fn stream_logs<R>(
    client: RunnerClient,
    job_id: i64,
    seq: std::sync::Arc<std::sync::atomic::AtomicI64>,
    stream: &'static str,
    mut reader: tokio::io::Lines<R>,
) where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut batch: Vec<LogLine> = Vec::with_capacity(LOG_BATCH_LINES);
    let mut ticker = interval(LOG_FLUSH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The first tick fires immediately; skip it so a batch that fills up
    // right away is not sent twice.
    ticker.tick().await;

    loop {
        tokio::select! {
            line = reader.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        let s = seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        batch.push(LogLine {
                            seq: s,
                            stream: stream.to_string(),
                            line,
                            timestamp: Utc::now().to_rfc3339(),
                        });
                        if batch.len() >= LOG_BATCH_LINES {
                            flush_logs(&client, job_id, &mut batch).await;
                        }
                    }
                    // End of stream: send what is left and stop.
                    Ok(None) => {
                        flush_logs(&client, job_id, &mut batch).await;
                        break;
                    }
                    Err(_) => {
                        flush_logs(&client, job_id, &mut batch).await;
                        break;
                    }
                }
            }
            _ = ticker.tick() => {
                flush_logs(&client, job_id, &mut batch).await;
            }
        }
    }
}

/// Send a batch of log lines. A failed send is logged and dropped: the
/// job's outcome must not depend on the log transport.
async fn flush_logs(client: &RunnerClient, job_id: i64, batch: &mut Vec<LogLine>) {
    if batch.is_empty() {
        return;
    }
    let lines = std::mem::take(batch);
    if let Err(e) = client.append_logs(job_id, &lines).await {
        tracing::warn!("could not send job logs: {e:#}");
    }
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
