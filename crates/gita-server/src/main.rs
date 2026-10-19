use clap::Parser;
use gita_server::{Args, Command, Config, WorkerCommand, run, worker_register};
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
    match args.command.unwrap_or(Command::Serve) {
        Command::Serve => {
            let config = Config::load(&args.config)?;
            run(config).await
        }
        Command::Worker { action } => match action {
            WorkerCommand::Register { name, user } => {
                let config = Config::load(&args.config)?;
                let token = worker_register(&config, &name, &user).await?;
                println!("runner token: {token}");
                println!("export GITA_RUNNER_TOKEN={token}");
                Ok(())
            }
        },
    }
}