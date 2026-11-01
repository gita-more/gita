//! `gita` — the user-facing CLI for the Gita server.
//!
//! This binary is the ONLY entry point for regular developers. It talks to
//! the server over the REST API with the shared [`gita_client`] transport
//! and covers everything a developer does day to day: authentication,
//! repositories, pull requests, issues, groups, search, CI pipelines and
//! SSH keys.
//!
//! Administrative and infrastructure commands deliberately live in the
//! separate `gita-admin` binary, so a developer never sees them.

mod args;
mod auth;
mod cmd;

use anyhow::Result;
use clap::CommandFactory;

pub use args::Args;
pub use gita_client::Config;

/// Parse the arguments and run the requested command.
pub async fn run(args: Args) -> Result<()> {
    // The credential helper must work without any configured server: git
    // asks it about arbitrary hosts, and its only job is to answer from
    // what is already stored.
    if let Some(args::Command::Auth {
        cmd: args::AuthCommand::Credential { operation },
    }) = &args.command
    {
        return auth::credential_helper(operation);
    }

    let config = Config::load(args.url.as_deref())?;
    let client = gita_client::GitaClient::new(config.server_url, config.token.clone())?;

    match args.command {
        Some(cmd) => match cmd {
            args::Command::Auth { cmd } => auth::dispatch(cmd, &client, args.json).await,
            cmd => cmd::dispatch(cmd, &client, args.json).await,
        },
        None => {
            Args::command().print_help()?;
            println!();
            Ok(())
        }
    }
}
