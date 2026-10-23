//! Gita CI runner — separate process that polls and executes pipeline runs.
//!
//! The runner connects directly to the shared Postgres database, claims
//! queued pipeline runs via FOR UPDATE SKIP LOCKED, clones the repo at
//! the pushed SHA, parses `.gita-ci.yml`, and executes each job in a
//! fresh workspace using `sh -c "<joined script>"`, writing line-buffered
//! stdout/stderr into the `logs` table as it goes.

pub mod claim;
pub mod config;
pub mod executor;
pub mod registry;
pub mod workspace;

use crate::claim::claim_one_run;
use crate::config::RunnerConfig;
use crate::executor::execute_pipeline;
use crate::registry::JobRegistry;
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

        // Execute the run in a spawned task so the polling loop can keep
        // claiming while this runs.
        let db = db.clone();
        let cfg = config.clone();
        let registry = JobRegistry::new();
        tokio::spawn(async move {
            if let Err(e) = execute_pipeline(&db, &cfg, &registry, &run).await {
                error!("execute_pipeline failed: {e:#}");
            }
        });
    }
}
