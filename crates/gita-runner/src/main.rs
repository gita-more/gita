//! The `gita-runner` binary.
//!
//! The runner polls the gita server for queued pipeline runs and executes
//! them. It needs two things: the server's URL and a runner token (a
//! personal access token named `runner/…`, minted by
//! `gita-admin worker register`).
//!
//! It deliberately needs **no database credentials**: the server is the
//! only component that talks to Postgres.

use clap::Parser;
use gita_runner::{config::RunnerConfig, run};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = RunnerConfig::parse();
    run(config).await
}
