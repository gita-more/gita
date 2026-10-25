//! Gita server: configuration, application assembly and startup.

pub mod config;

pub mod shutdown;

mod app;

mod runtime;

pub use config::{Config, ServerConfig, SshConfig};

pub use app::App;

pub use runtime::{run, sign_webhook_payload, worker_register};

use clap::Parser;

use std::path::PathBuf;

/// Command-line arguments.
#[derive(Debug, Parser)]
#[command(name = "gita", version, about = "AI-native Git server")]
pub struct Args {
    /// Subcommand; defaults to `serve` when omitted.
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Path to the configuration file (used by `serve`).
    #[arg(short, long, default_value = "config/gita.toml", global = true)]
    pub config: PathBuf,
}

/// Top-level commands.
#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Start the server (default).
    Serve,
    /// Register a CI runner and mint its token.
    Worker {
        #[command(subcommand)]
        action: WorkerCommand,
    },
}

/// Worker-related subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum WorkerCommand {
    /// Mint a runner token for a user and print it.
    Register {
        /// Human-readable runner name (used as the PAT name).
        #[arg(long)]
        name: String,
        /// Username whose account the runner will act as.
        #[arg(long)]
        user: String,
    },
}
