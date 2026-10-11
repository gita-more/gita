use clap::Parser;
use gita_server::{Args, Config, run};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,gita=debug"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();

    let args = Args::parse();
    let config = Config::load(&args.config)?;
    run(config).await
}