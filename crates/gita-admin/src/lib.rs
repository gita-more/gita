//! Gita admin: configuration, application assembly and startup.

pub mod config;
pub mod image;
pub mod runner;
pub mod template;
pub mod users;
pub mod vm;

pub mod app;
pub mod runtime;
pub mod shutdown;
pub mod webhook;

pub use app::App;
pub use config::{
    Args, Command, Config, ImageCmd, RunnerCmd, ServerConfig, SshConfig, TemplateCmd, VmCmd,
    WorkerCommand,
};
pub use runtime::{run, sign_webhook_payload, worker_register};
