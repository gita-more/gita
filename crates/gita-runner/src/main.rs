//! gita-runner: standalone CI runner binary.

use clap::Parser;
use gita_runner::config::RunnerConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,gita_runner=debug".into()),
        )
        .init();

    let config = RunnerConfig::parse();
    gita_runner::run(config).await
}
