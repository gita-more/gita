//! Shared REST client for the Gita server.
//!
//! This crate holds the pieces that both CLIs need: the typed HTTP
//! client (`api`), client configuration (`config`), the OS-keyring
//! credential store (`credential`) and the output helpers (`output`).
//!
//! It contains no command definitions, so `gita` (user CLI) and
//! `gita-admin` (admin CLI) can share the transport without sharing
//! their command surface.

pub mod api;
pub mod config;
pub mod credential;
pub mod output;

pub use api::GitaClient;
pub use config::Config;
