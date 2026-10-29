//! Gita Admin CLI - server, CI infrastructure, and runner management.
//!
//! This is the ONLY entry point for platform administrators.
//! Regular users should use `gita` instead.

use clap::Parser;
use gita_admin::{run, worker_register, image, template, runner, users, Args, Command, Config, WorkerCommand};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,gita=debug".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

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
        Command::Image { cmd } => image::run(cmd).await,
        Command::Template { cmd } => template::run(cmd).await,
        Command::Runner { cmd } => runner::run(cmd).await,
        Command::Users { cmd } => users::run(cmd).await,
    }
}