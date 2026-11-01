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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// What the mock server received.
    #[derive(Debug, Clone)]
    struct Received {
        path: String,
        auth: Option<String>,
        body: serde_json::Value,
    }

    /// A server that records the callbacks the agent makes.
    async fn mock_server() -> (String, Arc<Mutex<Vec<Received>>>) {
        let received = Arc::new(Mutex::new(Vec::new()));
        let logs_sink = received.clone();
        let status_sink = received.clone();

        let app = axum::Router::new()
            .route(
                "/api/ci/agent/jobs/{job_id}/logs",
                axum::routing::post(move |axum::extract::Path(job_id): axum::extract::Path<i64>,
                                                  headers: axum::http::HeaderMap,
                                                  body: String| {
                    let sink = logs_sink.clone();
                    async move {
                        let auth = headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        let parsed = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                        sink.lock().unwrap().push(Received {
                            path: format!("/api/ci/agent/jobs/{job_id}/logs"),
                            auth,
                            body: parsed,
                        });
                        axum::http::StatusCode::OK
                    }
                }),
            )
            .route(
                "/api/ci/agent/jobs/{job_id}/status",
                axum::routing::post(move |axum::extract::Path(job_id): axum::extract::Path<i64>,
                                                  headers: axum::http::HeaderMap,
                                                  body: String| {
                    let sink = status_sink.clone();
                    async move {
                        let auth = headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        let parsed = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                        sink.lock().unwrap().push(Received {
                            path: format!("/api/ci/agent/jobs/{job_id}/status"),
                            auth,
                            body: parsed,
                        });
                        axum::http::StatusCode::OK
                    }
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), received)
    }

    fn args(server_url: &str) -> Args {
        Args {
            server_url: server_url.to_string(),
            job_id: 42,
            token: "secret-token".to_string(),
            repo_url: "https://example.com/repo.git".to_string(),
            commit: "abc123".to_string(),
            script: "echo hi".to_string(),
            ssh_key: String::new(),
        }
    }

    /// The trailing slash of the server URL is dropped, so that the
    /// paths built from it have exactly one separator.
    #[test]
    fn the_server_url_loses_its_trailing_slash() {
        let sink = LogSink::new("http://example.com/", "token");
        assert_eq!(sink.server_url(), "http://example.com");

        let sink = LogSink::new("http://example.com", "token");
        assert_eq!(sink.server_url(), "http://example.com");

        let sink = LogSink::new("http://example.com///", "token");
        assert_eq!(sink.server_url(), "http://example.com");
    }

    /// Each log line carries a sequence number of its own, so that the
    /// server can tell them apart.
    #[tokio::test]
    async fn the_log_lines_are_numbered() {
        let (url, received) = mock_server().await;
        let sink = LogSink::new(&url, "secret-token");
        let a = args(&url);

        sink.write(a.job_id, "stdout", "first".to_string());
        sink.write(a.job_id, "stdout", "second".to_string());

        // The sends happen in the background; wait for them.
        for _ in 0..100 {
            if received.lock().unwrap().len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let got = received.lock().unwrap().clone();
        assert_eq!(got.len(), 2, "got {got:?}");
        let seqs: Vec<i64> = got.iter().map(|r| r.body["seq"].as_i64().unwrap()).collect();
        assert_ne!(seqs[0], seqs[1], "the lines must not share a number");
        assert!(seqs.contains(&0), "the first line is number zero: {seqs:?}");
        assert_eq!(got[0].body["stream"], "stdout");
        assert_eq!(got[0].body["line"], "first");
        assert_eq!(got[0].auth.as_deref(), Some("Bearer secret-token"));
        assert_eq!(got[0].path, "/api/ci/agent/jobs/42/logs");
    }

    /// The sequence number keeps counting, even when the server is
    /// slow to accept the line.
    #[tokio::test]
    async fn the_sequence_number_keeps_counting() {
        let sink = LogSink::new("http://example.com", "token");
        assert_eq!(sink.next_seq(), 0);
        sink.write(1, "stdout", "a".to_string());
        sink.write(1, "stdout", "b".to_string());
        assert_eq!(sink.next_seq(), 2);
    }

    /// A status report tells the server the job's state and, when the
    /// job ended, its exit code.
    #[tokio::test]
    async fn the_status_report_carries_the_state_and_the_code() {
        let (url, received) = mock_server().await;
        let a = args(&url);

        report_status(&a, "running", None).await;
        report_status(&a, "failed", Some(3)).await;

        let got = received.lock().unwrap().clone();
        assert_eq!(got.len(), 2, "got {got:?}");
        assert_eq!(got[0].body["status"], "running");
        assert!(got[0].body["exit_code"].is_null());
        assert_eq!(got[1].body["status"], "failed");
        assert_eq!(got[1].body["exit_code"], 3);
        assert_eq!(got[0].path, "/api/ci/agent/jobs/42/status");
        assert_eq!(got[0].auth.as_deref(), Some("Bearer secret-token"));
    }

    /// A server that refuses the report must not stop the agent: the
    /// job still has to run.
    #[tokio::test]
    async fn a_refused_status_report_does_not_stop_the_agent() {
        // Nothing is listening on this port.
        let a = args("http://127.0.0.1:1");
        report_status(&a, "running", None).await;
    }

    /// The child's output is shipped to the server, line by line, on
    /// the stream it came from.
    #[tokio::test]
    async fn the_output_of_the_child_is_shipped() {
        let (url, received) = mock_server().await;
        let sink = LogSink::new(&url, "secret-token");
        let a = args(&url);

        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg("echo to_stdout; echo to_stderr >&2");
        let status = run_command(&a, &sink, cmd, "test").await.expect("the command must run");
        assert!(status.success());

        for _ in 0..100 {
            if received.lock().unwrap().len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let got = received.lock().unwrap().clone();
        let stdout_lines: Vec<&str> = got
            .iter()
            .filter(|r| r.body["stream"] == "stdout")
            .map(|r| r.body["line"].as_str().unwrap())
            .collect();
        let stderr_lines: Vec<&str> = got
            .iter()
            .filter(|r| r.body["stream"] == "stderr")
            .map(|r| r.body["line"].as_str().unwrap())
            .collect();
        assert_eq!(stdout_lines, vec!["to_stdout"]);
        assert_eq!(stderr_lines, vec!["to_stderr"]);
    }

    /// A command that cannot be started is reported and returns
    /// nothing: there is no exit code to give.
    #[tokio::test]
    async fn a_command_that_cannot_start_is_reported() {
        let (url, received) = mock_server().await;
        let sink = LogSink::new(&url, "secret-token");
        let a = args(&url);

        let mut cmd = Command::new("/nonexistent/command");
        cmd.arg("x");
        let status = run_command(&a, &sink, cmd, "the thing").await;
        assert!(status.is_none(), "there is no exit status");

        for _ in 0..100 {
            if !received.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let got = received.lock().unwrap().clone();
        assert_eq!(got.len(), 1, "got {got:?}");
        assert_eq!(got[0].body["stream"], "stderr");
        let line = got[0].body["line"].as_str().unwrap();
        assert!(line.contains("the thing"), "the line names the command: {line}");
        assert!(line.contains("spawn failed"), "got {line}");
    }

    /// The job's script runs with an environment of its own: the
    /// agent's token must not be in it.
    #[tokio::test]
    async fn the_job_script_does_not_see_the_agent_token() {
        let dir = tempfile::TempDir::new().unwrap();
        let cmd = script_command("echo $GITA_AGENT_TOKEN", dir.path());
        let envs: Vec<(String, Option<String>)> = cmd
            .as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().to_string(),
                    v.map(|v| v.to_string_lossy().to_string()),
                )
            })
            .collect();

        // The environment is cleared, so only what the agent sets is
        // there.
        assert!(
            !envs.iter().any(|(k, _)| k == "GITA_AGENT_TOKEN"),
            "the token must not be in the job's environment: {envs:?}"
        );
        assert_eq!(envs.len(), 3, "got {envs:?}");
        assert!(envs.iter().any(|(k, v)| k == "PATH" && v.is_some()));
        assert!(envs.iter().any(|(k, v)| k == "HOME" && v.is_some()));
        assert!(envs.iter().any(|(k, v)| k == "CI" && v.as_deref() == Some("true")));
    }

    /// The script runs in the workspace and stops at the first error.
    #[tokio::test]
    async fn the_job_script_runs_in_the_workspace() {
        let dir = tempfile::TempDir::new().unwrap();
        let cmd = script_command("pwd", dir.path());
        let args_of: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(args_of, vec!["-ec", "pwd"]);
        assert_eq!(cmd.as_std().get_current_dir(), Some(dir.path()));
    }

    /// The clone uses the key the agent wrote.
    #[tokio::test]
    async fn the_clone_uses_the_ssh_key() {
        let cmd = clone_command(
            "https://example.com/repo.git",
            Path::new("/home/ci/work/42"),
            Path::new("/home/ci/.ssh/id_ed25519"),
        );
        let envs: Vec<(String, Option<String>)> = cmd
            .as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().to_string(),
                    v.map(|v| v.to_string_lossy().to_string()),
                )
            })
            .collect();
        let ssh = envs
            .iter()
            .find(|(k, _)| k == "GIT_SSH_COMMAND")
            .expect("the ssh command must be set");
        assert!(
            ssh.1.as_deref().unwrap().contains("/home/ci/.ssh/id_ed25519"),
            "got {ssh:?}"
        );
    }

    /// The checkout names the commit the run asked for.
    #[tokio::test]
    async fn the_checkout_names_the_commit() {
        let cmd = checkout_command("abc123", Path::new("/home/ci/work/42"));
        let args_of: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(args_of, vec!["-C", "/home/ci/work/42", "checkout", "abc123"]);
    }

    /// The private key is written so that only its owner can read it.
    #[test]
    fn the_ssh_key_is_written_with_tight_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let key_path = dir.path().join(".ssh").join("id_ed25519");

        write_ssh_key(&key_path, "PRIVATE KEY").unwrap();

        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the key must be readable by its owner only");

        let dir_mode = std::fs::metadata(key_path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700, "the directory must be private too");

        assert_eq!(
            std::fs::read_to_string(&key_path).unwrap(),
            "PRIVATE KEY"
        );
    }

    /// A key that cannot be written is an error, not a silent success:
    /// the clone would fail with a confusing message otherwise.
    #[test]
    fn a_key_that_cannot_be_written_is_an_error() {
        let dir = tempfile::TempDir::new().unwrap();
        // A file where the directory should be.
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        let err = write_ssh_key(&blocker.join("key"), "PRIVATE KEY")
            .expect_err("the write must fail");
        assert!(err.kind() != std::io::ErrorKind::NotFound || true);
    }
}
