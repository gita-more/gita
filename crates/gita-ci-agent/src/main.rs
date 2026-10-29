//! Gita CI Agent - runs inside the CI VM, polls for jobs and executes them.
//!
//! This binary is designed to be built as a static binary (musl target)
//! and embedded into the CI VM image. It runs as a systemd service
//! inside the VM and polls the orchestrator for jobs.

use clap::Parser;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::info;

#[derive(Parser)]
#[command(name = "gita-ci-agent", about = "Gita CI Agent - runs inside CI VM")]
struct Args {
    /// Orchestrator URL to poll for jobs
    #[arg(long, env = "GITA_CI_ORCHESTRATOR_URL")]
    orchestrator_url: String,

    /// Agent ID (defaults to hostname)
    #[arg(long, env = "GITA_CI_AGENT_ID")]
    agent_id: Option<String>,

    /// Poll interval in seconds
    #[arg(long, env = "GITA_CI_POLL_INTERVAL", default_value = "5")]
    poll_interval: u64,
}

struct DbLogSink {
    client: reqwest::Client,
    orchestrator_url: String,
}

impl DbLogSink {
    fn write(&self, job_id: i64, seq: i64, stream: &str, line: &str) {
        let client = self.client.clone();
        let url = format!("{}/api/v1/ci/jobs/{}/logs", self.orchestrator_url, job_id);
        let line = line.to_string();
        let stream = stream.to_string();
        tokio::spawn(async move {
            let _ = client
                .post(&url)
                .json(&serde_json::json!({
                    "seq": seq,
                    "stream": stream,
                    "line": line,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                }))
                .send()
                .await;
        });
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let agent_id = args.agent_id.unwrap_or_else(|| {
        std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown-agent".into())
    });

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,gita_ci_agent=debug".into()),
        )
        .init();

    info!("gita-ci-agent {} starting, orchestrator: {}", agent_id, args.orchestrator_url);

    let client = reqwest::Client::new();
    let poll_interval = std::time::Duration::from_secs(args.poll_interval);

    loop {
        // Register / report ready
        let _ = client
            .post(&format!("{}/api/v1/ci/agents/register", args.orchestrator_url))
            .json(&serde_json::json!({
                "agent_id": &agent_id,
                "status": "ready",
            }))
            .send()
            .await;

        // Poll for next job
        let job_resp = client
            .get(&format!("{}/api/v1/ci/agents/{}/jobs/next", args.orchestrator_url, agent_id))
            .send()
            .await;

        if let Ok(resp) = job_resp {
            if resp.status().is_success() {
                let job_data: Option<serde_json::Value> = resp.json().await.ok().flatten();
                if let Some(job) = job_data {
                    let job_id = job["job_id"].as_i64().unwrap_or(0);
                    let repo_url = job["repo_url"].as_str().unwrap_or("");
                    let commit = job["commit"].as_str().unwrap_or("");
                    let command = job["command"].as_str().unwrap_or("");
                    let ssh_key = job["ssh_private_key"].as_str().unwrap_or("");

                    if job_id > 0 && !repo_url.is_empty() && !commit.is_empty() {
                        info!("Agent {} claimed job {}", agent_id, job_id);
                        let _ = execute_job(
                            &args.orchestrator_url,
                            job_id,
                            repo_url,
                            commit,
                            command,
                            ssh_key,
                        )
                        .await;
                    }
                }
            }
        }

        tokio::time::sleep(poll_interval).await;
    }
}

async fn execute_job(
    orchestrator_url: &str,
    job_id: i64,
    repo_url: &str,
    commit: &str,
    command: &str,
    ssh_key: &str,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();

    // Write SSH key first, before any clone.
    if !ssh_key.is_empty() {
        std::fs::write("/home/ci/deploy.key", ssh_key)?;
        std::fs::set_permissions("/home/ci/deploy.key", std::fs::Permissions::from_mode(0o600))?;
    }

    // Report job started
    let _ = client
        .post(&format!("{}/api/v1/ci/jobs/{}/status", orchestrator_url, job_id))
        .json(&serde_json::json!({"status": "running"}))
        .send()
        .await;

    // Prepare workspace
    let work_dir = format!("/home/ci/work/{}", job_id);
    std::fs::create_dir_all(&work_dir)?;

    // Clone repo
    let mut cmd = Command::new("git");
    cmd.arg("clone")
        .arg(repo_url)
        .arg(&work_dir)
        .env("GIT_SSH_COMMAND", format!("ssh -i /home/ci/deploy.key -o StrictHostKeyChecking=no"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    let status = child.wait().await?;
    if !status.success() {
        let _ = report_job_result(orchestrator_url, job_id, "failed", None).await;
        return Ok(());
    }

    // Checkout commit
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(&work_dir).arg("checkout").arg(commit);
    let mut child = cmd.spawn()?;
    let status = child.wait().await?;
    if !status.success() {
        let _ = report_job_result(orchestrator_url, job_id, "failed", None).await;
        return Ok(());
    }

    // Execute command
    let mut cmd = Command::new("bash");
    cmd.arg("-lc")
        .arg(command)
        .current_dir(&work_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .env("HOME", &work_dir)
        .env("DATABASE_URL", "postgres://gita:gita@localhost:55432/gita")
        .env("GITA_TEST_DATABASE_URL", "postgres://gita:gita@localhost:55432/gita");

    let mut child = cmd.spawn()?;
    let seq = Arc::new(AtomicI64::new(0));

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let mut out_reader = BufReader::new(stdout).lines();
    let mut err_reader = BufReader::new(stderr).lines();

    let sink = Arc::new(DbLogSink {
        client: reqwest::Client::new(),
        orchestrator_url: orchestrator_url.to_string(),
    });

    let seq_out = seq.clone();
    let sink_out = sink.clone();
    let stdout_task = tokio::spawn(async move {
        while let Ok(Some(line)) = out_reader.next_line().await {
            let s = seq_out.fetch_add(1, Ordering::Relaxed);
            sink_out.write(job_id, s, "stdout", &line);
        }
    });
    let seq_err = seq.clone();
    let sink_err = sink.clone();
    let stderr_task = tokio::spawn(async move {
        while let Ok(Some(line)) = err_reader.next_line().await {
            let s = seq_err.fetch_add(1, Ordering::Relaxed);
            sink_err.write(job_id, s, "stderr", &line);
        }
    });

    let status = child.wait().await?;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), stdout_task).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), stderr_task).await;

    let exit_code = status.code();
    let success = status.success();
    let status_str = if success { "success" } else { "failed" };

    let _ = report_job_result(orchestrator_url, job_id, status_str, exit_code).await;

    Ok(())
}

async fn report_job_result(
    orchestrator_url: &str,
    job_id: i64,
    status: &str,
    exit_code: Option<i32>,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let _ = client
        .post(&format!("{}/api/v1/ci/jobs/{}/status", orchestrator_url, job_id))
        .json(&serde_json::json!({
            "status": status,
            "exit_code": exit_code,
        }))
        .send()
        .await;
    Ok(())
}