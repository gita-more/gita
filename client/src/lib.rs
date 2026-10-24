//! gita-client: a GitHub-gh-like CLI client for the Gita server.
//!
//! `run(args)` is the async entrypoint called by `src/main.rs`. The binary
//! itself is thin: it only parses args, initializes tracing, and dispatches.

mod api;
mod args;
mod auth;
mod cmd;
mod config;
mod credential;
mod output;

use anyhow::Result;
use clap::CommandFactory;

pub use args::Args;
pub use config::Config;

pub async fn run(args: Args) -> Result<()> {
    let config = Config::load(args.url.as_deref())?;
    let client = crate::api::GitaClient::new(config.server_url, config.token.clone())?;

    match args.command {
        Some(cmd) => match cmd {
            crate::args::Command::Auth { cmd } => {
                crate::auth::dispatch(cmd, &client, args.json).await
            }
            cmd => crate::cmd::dispatch(cmd, &client, args.json).await,
        },
        None => {
            Args::command().print_help()?;
            println!();
            Ok(())
        }
    }
}
