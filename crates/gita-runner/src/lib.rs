//! Gita CI runner — separate process that polls and executes CI runs.
//!
//! The runner talks to the gita server over HTTP and authenticates with a
//! **runner token** (a personal access token named `runner/…`). It asks
//! the server to claim queued runs, reports job status and streams logs
//! back through the API. It never touches the database: the server is the
//! only component that knows the schema, and the runner needs nothing but
//! a URL and a token to run.
//!
//! Two configuration formats are supported, distinguished by the pipeline
//! row's path:
//!
//! * `.gita-ci.yml` — gita's original GitLab-style pipeline, executed as
//!   one shell script per job.
//! * `.github/workflows/*.yml` — GitHub-Actions-style workflows, executed
//!   step by step (see [`workflow_run`]).

pub mod client;
pub mod clone;
pub mod config;
pub mod executor;
pub mod registry;
pub mod vm_exec;
pub mod workflow_exec;
pub mod workflow_pipeline;
pub mod workflow_run;
pub mod workspace;

use crate::client::RunnerClient;
use crate::config::RunnerConfig;
use crate::executor::execute_pipeline;
use crate::registry::JobRegistry;
use crate::workflow_run::{execute_workflow_run, is_workflow_path};
use anyhow::Context;
use tracing::{debug, error, info};

/// Run the runner event loop until the process is terminated.
pub async fn run(config: RunnerConfig) -> anyhow::Result<()> {
    let client = RunnerClient::new(&config.server_base_url, &config.runner_token)
        .with_context(|| "runner: build API client")?;

    info!(
        runner_name = %config.runner_name,
        server = %config.server_base_url,
        poll_interval = %config.poll_interval_secs,
        work_dir = %config.work_dir.display(),
        "gita-runner started"
    );

    let mut interval = tokio::time::interval(config.poll_interval());
    // First tick fires immediately; we want to start polling right away.
    interval.tick().await;

    loop {
        interval.tick().await;

        // Claim the next queued run. The server does the atomic claim, so
        // several runners polling at once still hand each run to exactly
        // one of them.
        let claimed = match client
            .claim_run(&config.runner_name, config.claim_lease_secs)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                error!("claim failed: {e:#}");
                continue;
            }
        };

        let Some(claimed) = claimed else {
            debug!("no queued runs");
            continue;
        };
        let run = claimed.run.expect("a claimed run carries its row");
        let Some(pipeline) = claimed.pipeline else {
            error!("run {} has no pipeline row; skipping", run.id);
            let _ = client.set_run_status(run.id, "failed").await;
            continue;
        };

        info!(
            run_id = run.id,
            ref_name = %run.ref_name,
            head_sha = %run.head_sha,
            "runner claimed run"
        );

        // The pipeline's path decides which configuration format the run
        // uses.
        let is_workflow = is_workflow_path(&pipeline.path);

        // Execute the run in a spawned task so the polling loop can keep
        // claiming while this runs.
        let client = client.clone();
        let cfg = config.clone();
        let registry = JobRegistry::new();
        tokio::spawn(async move {
            // Keep the claim's lease alive for the whole run. Without
            // this, the server cannot tell a long job from a dead runner,
            // and once the reap grace passes the run is failed and taken
            // away from a runner that is in fact still working.
            let renew = spawn_lease_renewer(client.clone(), cfg.clone(), run.id);
            let result = if is_workflow {
                execute_workflow_run(&client, &cfg, &registry, &run, &pipeline.content).await
            } else {
                execute_pipeline(&client, &cfg, &registry, &run).await
            };
            renew.abort();
            if let Err(e) = result {
                error!("run {} failed to execute: {e:#}", run.id);
            }
        });
    }
}

/// Renew a run's lease every third of its duration until aborted.
///
/// Renewing at a third of the lease leaves room for two failed attempts
/// before the lease lapses. A `renewed: false` answer means the server
/// moved the run on without us; the task stops renewing, but it cannot
/// stop the job — the executor notices the status change itself.
fn spawn_lease_renewer(
    client: RunnerClient,
    config: RunnerConfig,
    run_id: i64,
) -> tokio::task::JoinHandle<()> {
    let lease = config.claim_lease_secs.max(3);
    let interval = std::time::Duration::from_secs(lease / 3);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await; // the claim itself already took the lease
        loop {
            ticker.tick().await;
            match client
                .renew_run(run_id, &config.runner_name, lease)
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!(
                        run = run_id,
                        "lease renewal refused; the run no longer belongs to this runner"
                    );
                    return;
                }
                Err(e) => tracing::warn!(run = run_id, "lease renewal failed: {e:#}"),
            }
        }
    })
}
