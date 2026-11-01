//! Database-backed execution of a GitHub-Actions-style workflow run.
//!
//! This is the counterpart to [`crate::executor`]: where that module
//! runs a `.gita-ci.yml` pipeline, this one runs a
//! `.github/workflows/*.yml` workflow. Both share the same run/job/log
//! tables — a "pipeline" row whose `path` is a workflow file holds the
//! workflow's YAML, and the run's jobs are the workflow's jobs.
//!
//! The flow is:
//!
//! 1. Read the workflow YAML from the pipeline row.
//! 2. Parse and expand it (matrix → job instances, `needs` → order).
//! 3. For each job instance: insert a job row, execute its steps in the
//!    checked-out repository, stream logs into the `logs` table, and
//!    record the outcome.
//! 4. Mark the run `success` or `failed`.
//!
//! Job rows are named after the workflow job. A job with a matrix
//! produces several instances, so the instance's matrix label is appended
//! to keep the `(run_id, name)` uniqueness constraint satisfied.

use crate::client::{LogLine, RunnerClient};
use crate::clone::{prepare_run_repo, CloneOutcome};
use crate::config::RunnerConfig;
use crate::registry::JobRegistry;
use crate::workflow_exec::{execute_workflow_job, LogSink};
use anyhow::{Context, Result};
use chrono::Utc;
use gita_db::PipelineRunRow;
use gita_workflow::WorkflowJobInstance;
use std::sync::Arc;

/// The path prefix that marks a pipeline row as a workflow rather than a
/// `.gita-ci.yml` pipeline.
pub const WORKFLOW_PATH_PREFIX: &str = ".github/workflows/";

/// Whether a pipeline row's path designates a workflow file.
pub fn is_workflow_path(path: &str) -> bool {
    path.starts_with(WORKFLOW_PATH_PREFIX)
}

/// A log sink that sends lines to the server.
struct ApiLogSink {
    client: RunnerClient,
    job_id: i64,
}

impl LogSink for ApiLogSink {
    fn write(&self, job_id: i64, seq: i64, stream: &str, line: &str) {
        // The sink is called from the executor's reader tasks; the write
        // is fire-and-forget so a slow server does not stall the pipe.
        // The server's insert is idempotent per (job_id, seq), so a
        // duplicate is a no-op rather than an error.
        let client = self.client.clone();
        let job_id = if job_id == 0 { self.job_id } else { job_id };
        let line = line.to_string();
        let stream = stream.to_string();
        tokio::spawn(async move {
            let _ = client
                .append_logs(
                    job_id,
                    &[LogLine {
                        seq,
                        stream,
                        line,
                        timestamp: Utc::now().to_rfc3339(),
                    }],
                )
                .await;
        });
    }
}

/// The name recorded for a job instance.
///
/// A workflow job with a matrix expands into several instances that share
/// the job's name; the matrix label keeps them distinct (the `jobs` table
/// is unique on `(run_id, name)`).
pub fn instance_job_name(instance: &WorkflowJobInstance) -> String {
    if instance.matrix_label.is_empty() {
        instance.job.name.clone()
    } else {
        format!("{} {}", instance.job.name, instance.matrix_label)
    }
}

/// Execute a workflow run end to end.
///
/// The run's pipeline row must hold the workflow's YAML in its `content`
/// column. On any failure the run is marked `failed` rather than left
/// running.
pub async fn execute_workflow_run(
    client: &RunnerClient,
    config: &RunnerConfig,
    registry: &JobRegistry,
    run: &PipelineRunRow,
    content: &str,
) -> Result<()> {
    // ---- 1. The workflow YAML comes with the claim ----
    let content = content.to_string();

    // ---- 2. Parse and expand ----
    let spec = match crate::workflow_pipeline::prepare_workflow(&content) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("parse workflow failed: {e}");
            client.set_run_status(run.id, "failed").await?;
            return Ok(());
        }
    };

    // ---- 3. Prepare the checkout ----
    let repo_dir = match prepare_run_repo(client, config, run).await? {
        CloneOutcome::Ready(dir) => dir,
        CloneOutcome::Failed(reason) => {
            tracing::warn!("workflow checkout failed: {reason}");
            client.set_run_status(run.id, "failed").await?;
            return Ok(());
        }
    };

    // ---- 4. Execute each job instance ----
    let mut any_failed = false;
    let mut canceled = false;

    for instance in &spec.instances {
        // A canceled run must not start new jobs.
        if let Ok(fresh) = client.get_run(run.id).await {
            if fresh.status == "canceled" {
                canceled = true;
                break;
            }
        }

        let job_name = instance_job_name(instance);
        let job_id = client
            .create_job(run.id, &job_name, &instance.job.name, &[], &[])
            .await?;
        client.set_job_status(job_id, "running", None).await?;

        let sink = Arc::new(ApiLogSink {
            client: client.clone(),
            job_id,
        });

        let result = execute_workflow_job(instance, &repo_dir, job_id, registry, sink).await;

        match result {
            Ok(res) => {
                let status = if res.success { "success" } else { "failed" };
                client.set_job_status(job_id, status, res.exit_code).await?;
                if !res.success {
                    any_failed = true;
                }
            }
            Err(e) => {
                // The executor itself failed (e.g. a `uses:` step that is
                // not implemented yet). Record it as a failed job.
                tracing::warn!("workflow job {job_name} failed to execute: {e:#}");
                client.set_job_status(job_id, "failed", None).await?;
                any_failed = true;
            }
        }

        // Stop at the first failure — matching GitHub's fail-fast default
        // and the orchestration layer's behavior.
        if any_failed {
            break;
        }
    }

    // ---- 5. Record the run's outcome ----
    if canceled {
        // The run row is already 'canceled'; don't overwrite it.
        return Ok(());
    }
    let final_status = if any_failed { "failed" } else { "success" };
    client
        .set_run_status(run.id, final_status)
        .await
        .context("update workflow run status")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gita_workflow::{RunOn, Step, WorkflowJob};
    use std::collections::BTreeMap;

    fn instance(name: &str, matrix_label: &str) -> WorkflowJobInstance {
        WorkflowJobInstance {
            job: WorkflowJob {
                name: name.to_string(),
                runs_on: RunOn::Label("ubuntu-latest".to_string()),
                steps: vec![Step {
                    name: None,
                    run: Some("echo hi".to_string()),
                    uses: None,
                    env: BTreeMap::new(),
                    working_directory: None,
                    if_condition: None,
                }],
                needs: Vec::new(),
                env: BTreeMap::new(),
                matrix: None,
                shell: None,
            },
            matrix: BTreeMap::new(),
            matrix_label: matrix_label.to_string(),
        }
    }

    #[test]
    fn workflow_paths_are_recognized() {
        assert!(is_workflow_path(".github/workflows/ci.yml"));
        assert!(is_workflow_path(".github/workflows/release.yaml"));
        assert!(!is_workflow_path(".gita-ci.yml"));
        assert!(!is_workflow_path("ci.yml"));
    }

    #[test]
    fn a_plain_job_keeps_its_name() {
        let inst = instance("build", "");
        assert_eq!(instance_job_name(&inst), "build");
    }

    #[test]
    fn a_matrix_job_gets_its_label_appended() {
        let inst = instance("build", "(os: \"ubuntu\")");
        assert_eq!(instance_job_name(&inst), "build (os: \"ubuntu\")");
    }
}
