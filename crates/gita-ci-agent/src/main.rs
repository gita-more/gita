//! Gita CI Agent — one-shot job execution inside a CI VM.
//!
//! The runner provisions a VM for a `vm:` job and injects everything the
//! agent needs through cloud-init user data: which repository, which
//! commit, which command, and the per-job callback token. There is
//! nothing to poll: the VM boots with exactly one job to run, and the
//! agent's whole lifecycle is
//!
//! ```text
//! report running → clone → checkout → run script → report success/failed
//! ```
//!
//! (An earlier iteration polled `/api/v1/ci/agents/{id}/jobs/next`, but
//! no server ever implemented those endpoints — the polling loop was dead
//! code. One VM per job also gives the cleaner blast radius: a job's
//! environment is always fresh, and reclaiming the VM reclaims
//! everything about the job.)
//!
//! Every callback is best-effort: a dropped log line or a failed status
//! report must not fail the job. The runner watches the job row and
//! reclaims the VM once it reaches a terminal state (or its timeout),
//! so a crashed agent still leaves the pipeline in a consistent state.

use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::info;

#[derive(Parser)]
#[command(
    name = "gita-ci-agent",
    about = "Gita CI agent: runs one job inside a VM and reports back"
)]
struct Args {
    /// Base URL of the gita server (callbacks go to /api/ci/agent/...).
    #[arg(long, env = "GITA_AGENT_SERVER_URL")]
    server_url: String,

    /// The job this VM was created for.
    #[arg(long, env = "GITA_AGENT_JOB_ID")]
    job_id: i64,

    /// Per-job callback token minted by the runner.
    #[arg(long, env = "GITA_AGENT_TOKEN")]
    token: String,

    /// Repository URL to clone.
    #[arg(long, env = "GITA_AGENT_REPO_URL")]
    repo_url: String,

    /// Commit to check out.
    #[arg(long, env = "GITA_AGENT_COMMIT")]
    commit: String,

    /// The job's script, joined as one shell program.
    #[arg(long, env = "GITA_AGENT_SCRIPT")]
    script: String,

    /// Private key for cloning over SSH, if the repo is private.
    #[arg(long, env = "GITA_AGENT_SSH_KEY", default_value = "")]
    ssh_key: String,
}

/// Ship log lines to the server. Failures are swallowed: a log line the
/// server did not get is a gap in the output, not a failed job.
struct LogSink {
    client: reqwest::Client,
    server_url: String,
    token: String,
    seq: AtomicI64,
}

impl LogSink {
    fn new(server_url: &str, token: &str) -> Arc<Self> {
        Arc::new(Self {
            client: reqwest::Client::new(),
            server_url: server_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            seq: AtomicI64::new(0),
        })
    }

    fn write(self: &Arc<Self>, job_id: i64, stream: &'static str, line: String) {
        let sink = self.clone();
        let seq = sink.seq.fetch_add(1, Ordering::Relaxed);
        tokio::spawn(async move {
            let url = format!("{server}/api/ci/agent/jobs/{job_id}/logs", server = sink.server_url);
            let _ = sink
                .client
                .post(&url)
                .bearer_auth(&sink.token)
                .json(&serde_json::json!({
                    "seq": seq,
                    "stream": stream,
                    "line": line,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                }))
                .timeout(Duration::from_secs(5))
                .send()
                .await;
        });
    }
}

/// Report a status transition. Also best-effort, but awaited (not
/// spawned) so the process does not exit before the report leaves.
async fn report_status(args: &Args, status: &str, exit_code: Option<i32>) {
    let url = format!(
        "{}/api/ci/agent/jobs/{}/status",
        args.server_url.trim_end_matches('/'),
        args.job_id
    );
    let res = reqwest::Client::new()
        .post(&url)
        .bearer_auth(&args.token)
        .json(&serde_json::json!({ "status": status, "exit_code": exit_code }))
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    match res {
        Ok(resp) if resp.status().is_success() => {}
        Ok(resp) => tracing::warn!("status report to {url} returned {}", resp.status()),
        Err(e) => tracing::warn!("status report to {url} failed: {e}"),
    }
}

/// Stream a child's stdout/stderr into the sink. Returns when both pipes
/// are exhausted and the child has been reaped.
async fn run_command(
    args: &Args,
    sink: &Arc<LogSink>,
    mut cmd: Command,
    what: &'static str,
) -> Option<std::process::ExitStatus> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            sink.write(args.job_id, "stderr", format!("{what}: spawn failed: {e}"));
            return None;
        }
    };
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let job_id = args.job_id;
    let sink_out = sink.clone();
    let out_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            sink_out.write(job_id, "stdout", line);
        }
    });
    let sink_err = sink.clone();
    let err_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            sink_err.write(job_id, "stderr", line);
        }
    });
    let status = child.wait().await;
    let _ = out_task.await;
    let _ = err_task.await;
    Some(status.ok()?)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    info!(
        job = args.job_id,
        server = %args.server_url,
        "gita-ci-agent starting for one job"
    );

    let sink = LogSink::new(&args.server_url, &args.token);

    // Claim the job first: queued → running. If the report fails, keep
    // going anyway — the runner's timeout is the safety net.
    report_status(&args, "running", None).await;

    // Prepare the workspace.
    let work_dir = format!("/home/ci/work/{}", args.job_id);
    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        sink.write(args.job_id, "stderr", format!("create {work_dir}: {e}"));
        report_status(&args, "failed", None).await;
        anyhow::bail!("cannot create work dir: {e}");
    }

    // Private repos clone over SSH; write the key with tight permissions
    // before any git command runs.
    if !args.ssh_key.is_empty() {
        let key_path = "/home/ci/.ssh/id_ed25519";
        if let Some(parent) = std::path::Path::new(key_path).parent() {
            let _ = std::fs::create_dir_all(parent);
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
        if let Err(e) = std::fs::write(key_path, &args.ssh_key) {
            sink.write(args.job_id, "stderr", format!("write ssh key: {e}"));
        } else {
            let _ = std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600));
        }
    }

    // Clone + checkout the commit the run names.
    let mut clone = Command::new("git");
    clone
        .arg("clone")
        .arg(&args.repo_url)
        .arg(&work_dir)
        .env(
            "GIT_SSH_COMMAND",
            "ssh -i /home/ci/.ssh/id_ed25519 -o StrictHostKeyChecking=no",
        );
    let Some(status) = run_command(&args, &sink, clone, "git clone").await else {
        report_status(&args, "failed", None).await;
        anyhow::bail!("git clone could not run");
    };
    if !status.success() {
        report_status(&args, "failed", status.code()).await;
        anyhow::bail!("git clone failed");
    }

    let mut checkout = Command::new("git");
    checkout.arg("-C").arg(&work_dir).arg("checkout").arg(&args.commit);
    let Some(status) = run_command(&args, &sink, checkout, "git checkout").await else {
        report_status(&args, "failed", None).await;
        anyhow::bail!("git checkout could not run");
    };
    if !status.success() {
        report_status(&args, "failed", status.code()).await;
        anyhow::bail!("git checkout failed");
    }

    // Run the job's script. The environment is built from scratch so the
    // agent's own configuration (notably GITA_AGENT_TOKEN) never reaches
    // job code.
    let mut script_cmd = Command::new("bash");
    script_cmd
        .arg("-ec")
        .arg(&args.script)
        .current_dir(&work_dir)
        .env_clear()
        .env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        .env("HOME", &work_dir)
        .env("CI", "true");
    let Some(status) = run_command(&args, &sink, script_cmd, "script").await else {
        report_status(&args, "failed", None).await;
        anyhow::bail!("script shell could not run");
    };

    let code = status.code();
    if status.success() {
        info!(job = args.job_id, "job finished successfully");
        report_status(&args, "success", code).await;
        Ok(())
    } else {
        info!(job = args.job_id, ?code, "job failed");
        report_status(&args, "failed", code).await;
        // The exit code is the job's outcome, not an agent malfunction —
        // a non-zero script exit is a normal way for this process to end.
        std::process::exit(code.unwrap_or(1));
    }
}
