//! Command handler for a CLI domain.

use anyhow::Result;

use gita_client::api::GitaClient;
use crate::args;


pub(super) fn config(cmd: args::ConfigCommand, _client: &GitaClient, _json: bool) -> Result<()> {
    match cmd {
        args::ConfigCommand::SetUrl { url } => {
            gita_client::config::Config::save_url(&url)?;
            println!("Server URL set to {url}");
            Ok(())
        }
        args::ConfigCommand::Show => {
            println!("(config display not implemented)");
            Ok(())
        }
    }
}
