//! # gita-ci-agent
//!
//! The agent that runs one CI job inside a VM.
//!
//! Everything the agent does is best-effort: a log line the server did
//! not get is a gap in the output, not a failed job. The runner watches
//! the job row and reclaims the VM once it reaches a terminal state (or
//! its timeout), so a crashed agent still leaves the pipeline in a
//! consistent state.
//!
//! The pieces live here rather than in the binary so that they can be
//! tested: the binary is the wiring, this is the behaviour.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// The configuration of one agent run.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "gita-ci-agent",
    about = "Gita CI agent: runs one job inside a VM and reports back"
)]
pub struct Args {
    /// Base URL of the gita server (callbacks go to /api/ci/agent/...).
    #[arg(long, env = "GITA_AGENT_SERVER_URL")]
    pub server_url: String,

    /// The job this VM was created for.
    #[arg(long, env = "GITA_AGENT_JOB_ID")]
    pub job_id: i64,

    /// Per-job callback token minted by the runner.
    #[arg(long, env = "GITA_AGENT_TOKEN")]
    pub token: String,

    /// Repository URL to clone.
    #[arg(long, env = "GITA_AGENT_REPO_URL")]
    pub repo_url: String,

    /// Commit to check out.
    #[arg(long, env = "GITA_AGENT_COMMIT")]
    pub commit: String,

    /// The job's script, joined as one shell program.
    #[arg(long, env = "GITA_AGENT_SCRIPT")]
    pub script: String,

    /// Private key for cloning over SSH, if the repo is private.
    #[arg(long, env = "GITA_AGENT_SSH_KEY", default_value = "")]
    pub ssh_key: String,
}

/// Ship log lines to the server. Failures are swallowed: a log line the
/// server did not get is a gap in the output, not a failed job.
pub struct LogSink {
    client: reqwest::Client,
    server_url: String,
    token: String,
    seq: AtomicI64,
}

impl LogSink {
    pub fn new(server_url: &str, token: &str) -> Arc<Self> {
        Arc::new(Self {
            client: reqwest::Client::new(),
            server_url: server_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            seq: AtomicI64::new(0),
        })
    }

    /// The base URL the sink sends to: the trailing slash is gone, so
    /// that the paths built from it have exactly one separator.
    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    /// The sequence number the next line will carry.
    pub fn next_seq(&self) -> i64 {
        self.seq.load(Ordering::Relaxed)
    }

    /// Send one line. The line is sent in the background: the agent
    /// must not wait for the server to accept a log line.
    pub fn write(self: &Arc<Self>, job_id: i64, stream: &'static str, line: String) {
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
pub async fn report_status(args: &Args, status: &str, exit_code: Option<i32>) {
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
pub async fn run_command(
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

/// The command that runs the job's script.
///
/// The environment is built from scratch: the agent's own configuration
/// (notably the callback token) must never reach the job's code.
pub fn script_command(script: &str, work_dir: &Path) -> Command {
    let mut cmd = Command::new("bash");
    cmd.arg("-ec")
        .arg(script)
        .current_dir(work_dir)
        .env_clear()
        .env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        .env("HOME", work_dir)
        .env("CI", "true");
    cmd
}

/// The command that clones the repository.
pub fn clone_command(repo_url: &str, work_dir: &Path, ssh_key_path: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("clone")
        .arg(repo_url)
        .arg(work_dir)
        .env(
            "GIT_SSH_COMMAND",
            format!(
                "ssh -i {} -o StrictHostKeyChecking=no",
                ssh_key_path.display()
            ),
        );
    cmd
}

/// The command that checks the commit out.
pub fn checkout_command(commit: &str, work_dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(work_dir).arg("checkout").arg(commit);
    cmd
}

/// Write the private key the clone uses.
///
/// The directory is created with permissions that only its owner may
/// enter, and so is the key: ssh refuses a key that anyone else can
/// read.
pub fn write_ssh_key(key_path: &Path, key: &str) -> std::io::Result<()> {
    if let Some(parent) = key_path.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    std::fs::write(key_path, key)?;
    std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}
