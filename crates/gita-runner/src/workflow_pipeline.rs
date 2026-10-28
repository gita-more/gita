//! Orchestration of GitHub-Actions-style workflow runs.
//!
//! This layer ties [`gita_workflow`] parsing/expansion to the executor in
//! [`crate::workflow_exec`]. It is deliberately database-free: given a
//! workflow file's content and a checked-out repository, it produces the
//! ordered list of job instances and executes each one, streaming logs
//! through a [`crate::workflow_exec::LogSink`].
//!
//! Database persistence and event-driven triggering are layered on top by
//! the runner's main loop (see `executor`), keeping this module unit
//! testable without a server or database.

use crate::registry::JobRegistry;
use crate::workflow_exec::{execute_workflow_job, LogSink, WorkflowJobResult};
use anyhow::Result;
use gita_workflow::{expand, parse_workflow, WorkflowError, WorkflowSpec};
use std::path::Path;
use std::sync::Arc;

/// Parse and expand a workflow file's content into an executable plan.
///
/// This is a thin wrapper over `parse_workflow` + `expand`, kept so callers
/// (and tests) have a single entry point.
pub fn prepare_workflow(content: &str) -> Result<WorkflowSpec, WorkflowError> {
    let wf = parse_workflow(content)?;
    expand(wf)
}

/// Execute every job instance in a prepared workflow, in dependency order.
///
/// All jobs share `repo_dir` as their working directory (a simplification
/// of GitHub's per-job checkout: gita currently checks the repo out once
/// per run). `job_id_base` is used to derive unique job ids for the log
/// sink and registry. Returns one result per job instance, in execution
/// order.
pub async fn execute_prepared_workflow(
    spec: &WorkflowSpec,
    repo_dir: &Path,
    job_id_base: i64,
    registry: &JobRegistry,
    log_sink: Arc<dyn LogSink>,
) -> Result<Vec<WorkflowJobResult>> {
    let mut results = Vec::with_capacity(spec.instances.len());
    for (idx, instance) in spec.instances.iter().enumerate() {
        let job_id = job_id_base + idx as i64;
        let res =
            execute_workflow_job(instance, repo_dir, job_id, registry, log_sink.clone()).await?;
        let failed = !res.success;
        results.push(res);
        // Fail-fast: a failing job aborts the rest of the run, matching
        // GitHub's default `strategy.fail-fast` behavior. Dependents that
        // were waiting on the failed job never start.
        if failed {
            break;
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow_exec::{github_env, NullLogSink};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct VecLogSink(Arc<Mutex<Vec<(String, String)>>>);

    impl LogSink for VecLogSink {
        fn write(&self, _job_id: i64, _seq: i64, stream: &str, line: &str) {
            self.0
                .lock()
                .unwrap()
                .push((stream.to_string(), line.to_string()));
        }
    }

    #[test]
    fn prepare_parses_and_expands_matrix() {
        let content = r#"
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        os: [ubuntu, windows]
    steps:
      - run: echo ${{ matrix.os }}
"#;
        let spec = prepare_workflow(content).unwrap();
        assert_eq!(spec.instances.len(), 2);
        assert_eq!(spec.workflow.jobs.len(), 1);
    }

    #[test]
    fn prepare_detects_unknown_dependency() {
        let content = r#"
on: push
jobs:
  a:
    needs: missing
    runs-on: ubuntu-latest
    steps:
      - run: echo a
"#;
        let err = prepare_workflow(content).unwrap_err();
        assert!(matches!(err, WorkflowError::UnknownDependency(_, _)));
    }

    #[tokio::test]
    async fn execute_prepared_runs_all_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let registry = JobRegistry::new();
        let sink = Arc::new(NullLogSink);
        let content = r#"
on: push
jobs:
  a:
    runs-on: ubuntu-latest
    steps:
      - run: echo "hello-a"
  b:
    needs: a
    runs-on: ubuntu-latest
    steps:
      - run: echo "hello-b"
"#;
        let spec = prepare_workflow(content).unwrap();
        let results = execute_prepared_workflow(&spec, dir.path(), 1, &registry, sink)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.success));
        assert_eq!(results[0].job_name, "a");
        assert_eq!(results[1].job_name, "b");
    }

    #[tokio::test]
    async fn execute_prepared_stops_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let registry = JobRegistry::new();
        let sink = Arc::new(NullLogSink);
        let content = r#"
on: push
jobs:
  a:
    runs-on: ubuntu-latest
    steps:
      - run: exit 2
  b:
    needs: a
    runs-on: ubuntu-latest
    steps:
      - run: echo "never"
"#;
        let spec = prepare_workflow(content).unwrap();
        let results = execute_prepared_workflow(&spec, dir.path(), 1, &registry, sink)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
    }

    #[tokio::test]
    async fn execute_prepared_streams_logs() {
        let dir = tempfile::tempdir().unwrap();
        let registry = JobRegistry::new();
        let sink = Arc::new(VecLogSink::default());
        let content = r#"
on: push
jobs:
  a:
    runs-on: ubuntu-latest
    steps:
      - run: echo "hello-from-a"
"#;
        let spec = prepare_workflow(content).unwrap();
        let results = execute_prepared_workflow(&spec, dir.path(), 1, &registry, sink.clone())
            .await
            .unwrap();
        assert!(results[0].success);
        let lines = sink.0.lock().unwrap();
        assert!(lines.iter().any(|(_, l)| l.contains("hello-from-a")));
    }

    // Keep github_env referenced so the type is exercised in this module.
    fn _assert_github_env(_: BTreeMap<String, String>) {}
    fn _env_type() {
        let _ = github_env("r", "1", "j", "s", "r");
    }
}
