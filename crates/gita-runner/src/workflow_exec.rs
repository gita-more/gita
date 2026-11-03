//! Execution of GitHub-Actions-style workflow jobs.
//!
//! This is a self-contained executor for a [`WorkflowJobInstance`]: it runs
//! each `run:` step as a child shell process, injects job/step environment
//! variables and `GITHUB_*` variables, and streams stdout/stderr through a
//! caller-provided log sink. It deliberately knows nothing about the
//! database or the network — the caller supplies the job definition and the
//! log sink, so the whole module is unit-testable with real shell commands.
//!
//! `uses:` steps are not implemented yet; a job containing one is rejected
//! with a clear error rather than silently skipped.

use crate::registry::JobRegistry;
use anyhow::{Context, Result};
use gita_workflow::WorkflowJobInstance;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// A sink for log lines produced while executing a job. The runner wires
/// this to its database log writer; tests can collect lines into a Vec.
///
/// The trait is synchronous by nature but called from async contexts; the
/// default implementation is a no-op. Implementations that need to do I/O
/// should buffer and flush asynchronously.
pub trait LogSink: Send + Sync {
    /// Record one log line. Implementations must be cheap and non-blocking;
    /// the executor does not await this beyond the call itself.
    fn write(&self, job_id: i64, seq: i64, stream: &str, line: &str);
}

/// A no-op log sink (used when the caller only cares about exit codes).
pub struct NullLogSink;

impl LogSink for NullLogSink {
    fn write(&self, _job_id: i64, _seq: i64, _stream: &str, _line: &str) {}
}

/// Result of executing one workflow job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowJobResult {
    /// The job's name.
    pub job_name: String,
    /// Whether every step succeeded.
    pub success: bool,
    /// The exit code of the first failing step, if any.
    pub exit_code: Option<i32>,
}

/// Standard `GITHUB_*` environment variables injected into every step.
///
/// The set mirrors what GitHub Actions exposes, restricted to values gita
/// knows about. Values are provided by the caller so the executor stays
/// generic; the `GITHUB_` prefix is constant.
pub fn github_env(
    repo: &str,
    run_id: &str,
    job_name: &str,
    commit_sha: &str,
    ref_name: &str,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("GITHUB_REPOSITORY".to_string(), repo.to_string());
    env.insert("GITHUB_RUN_ID".to_string(), run_id.to_string());
    env.insert("GITHUB_JOB".to_string(), job_name.to_string());
    env.insert("GITHUB_SHA".to_string(), commit_sha.to_string());
    env.insert("GITHUB_REF".to_string(), ref_name.to_string());
    env.insert("GITHUB_WORKSPACE".to_string(), "/workspace".to_string());
    env.insert("CI".to_string(), "true".to_string());
    env
}

/// Execute a single workflow job in `work_dir`.
///
/// `repo_dir` is the checked-out repository; each step runs with it as the
/// working directory (or `working-directory` relative to it). `job_id` is
/// passed to the log sink for correlation. `registry` is used so a cancel
/// watcher can kill the running step; it is optional for tests that do not
/// need cancellation.
pub async fn execute_workflow_job(
    job_instance: &WorkflowJobInstance,
    repo_dir: &Path,
    job_id: i64,
    registry: &JobRegistry,
    log_sink: Arc<dyn LogSink>,
) -> Result<WorkflowJobResult> {
    let job = &job_instance.job;

    // Build the merged environment: job env overrides GITHUB_*, step env
    // overrides job env.
    let mut env = github_env(
        "gita/local",
        &job_id.to_string(),
        &job.name,
        "HEAD",
        "refs/heads/main",
    );
    for (k, v) in &job.env {
        env.insert(k.clone(), v.clone());
    }

    // Evaluate a job-level `if:` condition.
    if let Some(cond) = &job.if_condition {
        let ctx = gita_workflow::EvalContext {
            matrix: job_instance.matrix.clone(),
            env: env.clone(),
        };
        if !gita_workflow::evaluate_condition(cond, &ctx) {
            return Ok(WorkflowJobResult {
                job_name: job.name.clone(),
                success: true,
                exit_code: None,
            });
        }
    }

    let seq = Arc::new(AtomicI64::new(0));
    let mut exit_code: Option<i32> = None;

    for (idx, step) in job.steps.iter().enumerate() {
        // `uses:` steps: support the checkout action as a no-op (the
        // runner has already checked the repository out), and reject
        // anything else with a clear error.
        if let Some(uses) = &step.uses {
            if uses.starts_with("actions/checkout") {
                continue;
            }
            return Err(anyhow::anyhow!(
                "job '{}' step {} uses '{}': `uses:` actions are not implemented yet",
                job.name,
                idx + 1,
                uses
            ));
        }
        let run_cmd = match &step.run {
            Some(r) => r.clone(),
            None => continue,
        };

        // Evaluate a step-level `if:` condition.
        if let Some(cond) = &step.if_condition {
            let ctx = gita_workflow::EvalContext {
                matrix: job_instance.matrix.clone(),
                env: env.clone(),
            };
            if !gita_workflow::evaluate_condition(cond, &ctx) {
                continue;
            }
        }

        // Merge step env on top.
        let mut step_env = env.clone();
        for (k, v) in &step.env {
            step_env.insert(k.clone(), v.clone());
        }

        // Substitute `${{ ... }}` expressions before the command reaches
        // the shell. A raw `${{ matrix.os }}` is a syntax error in bash,
        // so a matrix job would fail on its first step without this.
        let eval_ctx = gita_workflow::EvalContext {
            matrix: job_instance.matrix.clone(),
            env: step_env.clone(),
        };
        let run_cmd = gita_workflow::evaluate(&run_cmd, &eval_ctx);

        let shell = job.shell.as_deref().unwrap_or("bash");

        // Resolve working directory.
        let cwd = match &step.working_directory {
            Some(rel) => repo_dir.join(rel),
            None => repo_dir.to_path_buf(),
        };

        let ok = run_one_step(
            &run_cmd,
            &cwd,
            &step_env,
            shell,
            job_id,
            seq.clone(),
            idx,
            registry,
            log_sink.clone(),
        )
        .await?;
        if !ok {
            exit_code = Some(1);
            break;
        }
    }

    Ok(WorkflowJobResult {
        job_name: job.name.clone(),
        success: exit_code.is_none(),
        exit_code,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_one_step(
    run_cmd: &str,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    shell: &str,
    job_id: i64,
    seq: Arc<AtomicI64>,
    step_index: usize,
    registry: &JobRegistry,
    log_sink: Arc<dyn LogSink>,
) -> Result<bool> {
    let key = format!("step-{step_index}");

    // GitHub runs `run:` through the job's (or step's) shell.
    let mut cmd = match shell {
        "python" | "python3" => {
            let mut c = Command::new("python3");
            c.arg("-c").arg(run_cmd);
            c
        }
        "sh" => {
            let mut c = Command::new("sh");
            c.arg("-c").arg(run_cmd);
            c
        }
        other => {
            let mut c = Command::new(other);
            c.arg("-c").arg(run_cmd);
            c
        }
    };
    cmd.current_dir(cwd).stdin(Stdio::null());

    // Build a clean environment from scratch: do not leak the runner's own
    // env (which holds its runner token) into the job.
    cmd.env_clear();
    cmd.env(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| {
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string()
        }),
    );
    cmd.env("HOME", cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    // Remove the runner's own credentials explicitly. A workflow may set
    // `DATABASE_URL` for its own use, so only the runner's variables are
    // stripped here.
    for secret in ["GITA_RUNNER_TOKEN", "GITA_SERVER_URL"] {
        cmd.env_remove(secret);
    }

    // Wire stdout/stderr.
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let child = cmd.spawn().context("spawn step shell")?;

    // Register the child so a cancel watcher can kill it.
    registry.register(job_id, &key, child).await;
    let mut child = registry
        .take(job_id, &key)
        .await
        .ok_or_else(|| anyhow::anyhow!("step child disappeared from registry"))?;

    // Drain stdout/stderr into the log sink.
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let mut out_reader = BufReader::new(stdout).lines();
    let mut err_reader = BufReader::new(stderr).lines();

    let seq_out = seq.clone();
    let log_sink_out = log_sink.clone();
    let stdout_task = tokio::spawn(async move {
        while let Ok(Some(line)) = out_reader.next_line().await {
            let s = seq_out.fetch_add(1, Ordering::Relaxed);
            log_sink_out.write(job_id, s, "stdout", &line);
        }
    });
    let seq_err = seq.clone();
    let log_sink_err = log_sink.clone();
    let stderr_task = tokio::spawn(async move {
        while let Ok(Some(line)) = err_reader.next_line().await {
            let s = seq_err.fetch_add(1, Ordering::Relaxed);
            log_sink_err.write(job_id, s, "stderr", &line);
        }
    });

    let status = child.wait().await.context("wait for step")?;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), stdout_task).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), stderr_task).await;

    Ok(status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gita_workflow::{RunOn, Step, WorkflowJob, WorkflowJobInstance};
    use std::sync::Mutex;

    /// A log sink that collects lines into a shared Vec for assertions.
    #[derive(Clone, Default)]
    struct VecLogSink(Arc<Mutex<Vec<(i64, i64, String, String)>>>);

    impl LogSink for VecLogSink {
        fn write(&self, job_id: i64, seq: i64, stream: &str, line: &str) {
            self.0
                .lock()
                .unwrap()
                .push((job_id, seq, stream.to_string(), line.to_string()));
        }
    }

    fn make_job(steps: Vec<Step>) -> WorkflowJobInstance {
        let job = WorkflowJob {
            name: "test".to_string(),
            runs_on: RunOn::Label("ubuntu-latest".to_string()),
            steps,
            needs: Vec::new(),
            env: BTreeMap::new(),
            matrix: None,
            shell: None,
            if_condition: None,
            services: BTreeMap::new(),
            container: None,
        };
        WorkflowJobInstance {
            job,
            matrix: BTreeMap::new(),
            matrix_label: String::new(),
        }
    }

    fn step(run: &str) -> Step {
        Step {
            name: None,
            run: Some(run.to_string()),
            uses: None,
            env: BTreeMap::new(),
            working_directory: None,
            if_condition: None,
        }
    }

    #[tokio::test]
    async fn runs_single_step_and_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let registry = JobRegistry::new();
        let sink_inner = Arc::new(VecLogSink::default());
        let sink: Arc<dyn LogSink> = sink_inner.clone();
        let job = make_job(vec![step("echo hello-world")]);
        let res = execute_workflow_job(&job, dir.path(), 1, &registry, sink.clone())
            .await
            .unwrap();
        assert!(res.success);
        assert_eq!(res.exit_code, None);
        let lines = sink_inner.0.lock().unwrap();
        assert!(lines
            .iter()
            .any(|(_, _, s, l)| s == "stdout" && l.contains("hello-world")));
    }

    #[tokio::test]
    async fn failing_step_reports_failure() {
        let dir = tempfile::tempdir().unwrap();
        let registry = JobRegistry::new();
        let sink_inner = Arc::new(VecLogSink::default());
        let sink: Arc<dyn LogSink> = sink_inner.clone();
        let job = make_job(vec![step("exit 3")]);
        let res = execute_workflow_job(&job, dir.path(), 1, &registry, sink.clone())
            .await
            .unwrap();
        assert!(!res.success);
        assert_eq!(res.exit_code, Some(1));
    }

    #[tokio::test]
    async fn multiple_steps_run_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let registry = JobRegistry::new();
        let sink_inner = Arc::new(VecLogSink::default());
        let sink: Arc<dyn LogSink> = sink_inner.clone();
        let job = make_job(vec![step("echo first"), step("echo second")]);
        let res = execute_workflow_job(&job, dir.path(), 1, &registry, sink.clone())
            .await
            .unwrap();
        assert!(res.success);
        let lines = sink_inner.0.lock().unwrap();
        let stdout: Vec<&str> = lines
            .iter()
            .filter(|(_, _, s, _)| s == "stdout")
            .map(|(_, _, _, l)| l.as_str())
            .collect();
        assert!(stdout.iter().any(|l| l.contains("first")));
        assert!(stdout.iter().any(|l| l.contains("second")));
        let i_first = stdout.iter().position(|l| l.contains("first")).unwrap();
        let i_second = stdout.iter().position(|l| l.contains("second")).unwrap();
        assert!(i_first < i_second);
    }

    #[tokio::test]
    async fn failure_stops_later_steps() {
        let dir = tempfile::tempdir().unwrap();
        let registry = JobRegistry::new();
        let sink_inner = Arc::new(VecLogSink::default());
        let sink: Arc<dyn LogSink> = sink_inner.clone();
        let job = make_job(vec![step("exit 1"), step("echo should-not-run")]);
        let res = execute_workflow_job(&job, dir.path(), 1, &registry, sink.clone())
            .await
            .unwrap();
        assert!(!res.success);
        let lines = sink_inner.0.lock().unwrap();
        assert!(!lines
            .iter()
            .any(|(_, _, _, l)| l.contains("should-not-run")));
    }

    #[tokio::test]
    async fn working_directory_is_honored() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let registry = JobRegistry::new();
        let sink_inner = Arc::new(VecLogSink::default());
        let sink: Arc<dyn LogSink> = sink_inner.clone();
        let job = make_job(vec![Step {
            name: None,
            run: Some("pwd".to_string()),
            uses: None,
            env: BTreeMap::new(),
            working_directory: Some("sub".to_string()),
            if_condition: None,
        }]);
        let res = execute_workflow_job(&job, dir.path(), 1, &registry, sink.clone())
            .await
            .unwrap();
        assert!(res.success);
        let lines = sink_inner.0.lock().unwrap();
        assert!(lines
            .iter()
            .any(|(_, _, s, l)| s == "stdout" && l.contains("sub")));
    }

    #[tokio::test]
    async fn step_env_is_injected() {
        let dir = tempfile::tempdir().unwrap();
        let registry = JobRegistry::new();
        let sink_inner = Arc::new(VecLogSink::default());
        let sink: Arc<dyn LogSink> = sink_inner.clone();
        let job = make_job(vec![Step {
            name: None,
            run: Some("echo $STEP_VAR".to_string()),
            uses: None,
            env: BTreeMap::from_iter([("STEP_VAR".to_string(), "magic".to_string())]),
            working_directory: None,
            if_condition: None,
        }]);
        let res = execute_workflow_job(&job, dir.path(), 1, &registry, sink.clone())
            .await
            .unwrap();
        assert!(res.success);
        let lines = sink_inner.0.lock().unwrap();
        assert!(lines.iter().any(|(_, _, _, l)| l.contains("magic")));
    }

    #[tokio::test]
    async fn uses_step_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let registry = JobRegistry::new();
        let sink_inner = Arc::new(VecLogSink::default());
        let sink: Arc<dyn LogSink> = sink_inner.clone();
        let job = make_job(vec![Step {
            name: None,
            run: None,
            uses: Some("foo/bar@v1".to_string()),
            env: BTreeMap::new(),
            working_directory: None,
            if_condition: None,
        }]);
        let res = execute_workflow_job(&job, dir.path(), 1, &registry, sink.clone()).await;
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("not implemented"));
    }

    /// A matrix value reaches the step as a substituted value, not as the
    /// literal `${{ matrix.os }}` — which bash would reject as a syntax
    /// error.
    #[tokio::test]
    async fn matrix_expressions_are_substituted_before_the_shell_sees_them() {
        let dir = tempfile::tempdir().unwrap();
        let registry = JobRegistry::new();
        let sink_inner = Arc::new(VecLogSink::default());
        let sink: Arc<dyn LogSink> = sink_inner.clone();

        let mut job = make_job(vec![step("echo running-on-${{ matrix.os }}")]);
        job.matrix = BTreeMap::from_iter([(
            "os".to_string(),
            serde_json::Value::String("ubuntu".to_string()),
        )]);

        let res = execute_workflow_job(&job, dir.path(), 1, &registry, sink.clone())
            .await
            .unwrap();
        assert!(
            res.success,
            "the step should not fail on an unresolved expression"
        );

        let lines = sink_inner.0.lock().unwrap();
        let text: String = lines
            .iter()
            .filter(|(_, _, s, _)| s == "stdout")
            .map(|(_, _, _, l)| l.as_str())
            .collect();
        assert!(
            text.contains("running-on-ubuntu"),
            "the matrix value should be substituted; output: {text}"
        );
        assert!(
            !text.contains("${{"),
            "no expression should survive into the shell; output: {text}"
        );
    }
}
