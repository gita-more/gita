//! Gita CI runner — separate process that polls and executes CI runs.
//!
//! The runner connects directly to the shared Postgres database, claims
//! queued runs via FOR UPDATE SKIP LOCKED, clones the repo at the pushed
//! SHA, and executes the run's jobs in a fresh workspace, writing
//! line-buffered stdout/stderr into the `logs` table as it goes.
//!
//! Two configuration formats are supported, distinguished by the pipeline
//! row's path:
//!
//! * `.gita-ci.yml` — gita's original GitLab-style pipeline, executed as
//!   one shell script per job.
//! * `.github/workflows/*.yml` — GitHub-Actions-style workflows, executed
//!   step by step (see [`workflow_run`]).

pub mod claim;
pub mod clone;
pub mod config;
pub mod executor;
pub mod registry;
pub mod workflow_exec;
pub mod workflow_pipeline;
pub mod workflow_run;
pub mod workspace;

use crate::claim::claim_one_run;
use crate::config::RunnerConfig;
use crate::executor::execute_pipeline;
use crate::registry::JobRegistry;
use crate::workflow_run::{execute_workflow_run, is_workflow_path};
use anyhow::Context;
use gita_db::Db;
use tracing::{debug, error, info};

/// Run the runner event loop until the process is terminated.
pub async fn run(config: RunnerConfig) -> anyhow::Result<()> {
    let db = Db::connect(&config.server_db_url)
        .await
        .with_context(|| "runner: connect to database")?;

    info!(
        runner_name = %config.runner_name,
        poll_interval = %config.poll_interval_secs,
        work_dir = %config.work_dir.display(),
        "gita-runner started"
    );

    let mut interval = tokio::time::interval(config.poll_interval());
    // First tick fires immediately; we want to start polling right away.
    interval.tick().await;

    loop {
        interval.tick().await;

        // Claim the next queued run. Multiple runner processes race via
        // SKIP LOCKED, so this is safe.
        let claimed = match claim_one_run(&db, &config.runner_name, config.claim_lease()).await {
            Ok(c) => c,
            Err(e) => {
                error!("claim failed: {e:#}");
                continue;
            }
        };

        let Some(run) = claimed else {
            debug!("no queued runs");
            continue;
        };

        info!(
            run_id = run.id,
            ref_name = %run.ref_name,
            head_sha = %run.head_sha,
            "runner claimed run"
        );

        // The pipeline row decides which configuration format the run
        // uses. A missing row means there is nothing to execute.
        let pipeline_path = match db.get_pipeline(run.pipeline_id).await {
            Ok(Some(p)) => p.path,
            Ok(None) => {
                error!("run {} has no pipeline row; skipping", run.id);
                let _ = db.update_pipeline_run_status(run.id, "failed").await;
                continue;
            }
            Err(e) => {
                error!("could not load pipeline for run {}: {e:#}", run.id);
                continue;
            }
        };
        let is_workflow = is_workflow_path(&pipeline_path);

        // Execute the run in a spawned task so the polling loop can keep
        // claiming while this runs.
        let db = db.clone();
        let cfg = config.clone();
        let registry = JobRegistry::new();
        tokio::spawn(async move {
            let result = if is_workflow {
                execute_workflow_run(&db, &cfg, &registry, &run).await
            } else {
                execute_pipeline(&db, &cfg, &registry, &run).await
            };
            if let Err(e) = result {
                error!("run {} failed to execute: {e:#}", run.id);
            }
        });
    }
}
