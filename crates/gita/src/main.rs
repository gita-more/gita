//! Thin binary entrypoint for the `gita` user CLI.

use clap::Parser;
use gita::run;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,gita_client=debug".into()),
        )
        .init();

    let args = gita::Args::parse();
    run(args).await
}
