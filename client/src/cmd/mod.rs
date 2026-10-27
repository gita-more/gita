//! Command handlers, split by domain.
//!
//! `dispatch` maps each `args::Command` variant to the appropriate handler.
//! Shared output/helper functions used by multiple domains live here.


use anyhow::{bail, Result};

use crate::api::GitaClient;
use crate::args;
use crate::credential;
use crate::output;


mod admin;
mod api_call;
mod config;
mod event;
mod group;
mod issue;
mod pipeline;
mod pr;
mod repo;
mod search;
mod ssh_key;

use self::admin::admin;
use self::api_call::call_api;
use self::config::config;
use self::event::event;
use self::group::group;
use self::issue::issue;
use self::pipeline::pipeline;
use self::pr::pr;
use self::repo::repo;
use self::search::search;
use self::ssh_key::ssh_key;



pub async fn dispatch(cmd: args::Command, client: &GitaClient, json: bool) -> Result<()> {
    match cmd {
        args::Command::Auth { cmd } => crate::auth::dispatch(cmd, client, json).await,
        args::Command::Repo { cmd } => repo(cmd, client, json).await,
        args::Command::Pr { cmd } => pr(cmd, client, json).await,
        args::Command::Issue { cmd } => issue(cmd, client, json).await,
        args::Command::Group { cmd } => group(cmd, client, json).await,
        args::Command::Search { cmd } => search(cmd, client, json).await,
        args::Command::Pipeline { cmd } => pipeline(cmd, client, json).await,
        args::Command::SshKey { cmd } => ssh_key(cmd, client, json).await,
        args::Command::Event { cmd } => event(cmd, client, json).await,
        args::Command::Admin { cmd } => admin(cmd, client, json).await,
        args::Command::Api { path, method, body } => {
            call_api(client, &path, &method, body.as_deref(), json).await
        }
        args::Command::Config { cmd } => config(cmd, client, json),
    }
}


pub(crate) fn print_repo(r: &crate::api::types::Repository, json: bool) {
    if json {
        let _ = output::print_json(r);
    } else {
        println!("full_name: {}", r.full_name);
        println!("description: {}", r.description);
        println!("private: {}", r.is_private);
        println!("default_branch: {}", r.default_branch);
        println!("created_at: {}", r.created_at);
    }
}


pub(crate) fn print_star(s: &crate::api::types::StarResponse) {
    println!(
        "starred: {} (count: {})",
        s.starred,
        s.new
            .map(|n| if n { "added" } else { "already starred" })
            .unwrap_or("unchanged")
    );
}


pub(crate) fn size_str(size: &Option<i64>) -> String {
    match size {
        Some(n) => n.to_string(),
        None => "-".to_string(),
    }
}


pub(crate) async fn clone_repo(client: &GitaClient, repo: &str, dir: Option<&std::path::Path>) -> Result<()> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("repo must be 'owner/name', got {repo}"))?;
    let (username, token) = match &client.token {
        Some(token) => (credential_username(client, owner)?, token.clone()),
        None => (String::new(), String::new()),
    };
    let origin = origin_of(&client.base_url)?;
    let (scheme, host) = origin.split_once("://").unwrap_or(("", &origin));
    let url = if token.is_empty() {
        format!("{scheme}://{host}/{owner}/{name}.git")
    } else {
        format!("{scheme}://{username}:{token}@{host}/{owner}/{name}.git")
    };
    let mut git = std::process::Command::new("git");
    git.arg("clone").arg(&url);
    if scheme == "http" {
        git.env("GIT_SSL_NO_VERIFY", "1");
    }
    if let Some(d) = dir {
        git.arg(d);
    }
    let status = git.status()?;
    if !status.success() {
        bail!("git clone failed with exit code {:?}", status.code());
    }
    let dest = dir
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    println!("Cloned {repo} into {dest}");
    Ok(())
}


pub(crate) fn origin_of(base_url: &str) -> Result<String> {
    let u: url::Url = base_url
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid base URL {base_url}: {e}"))?;
    let host = u
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("no host in {base_url}"))?;
    let port = u.port_or_known_default();
    match port {
        Some(p) if p == 80 && u.scheme() == "http" => Ok(format!("{}://{}", u.scheme(), host)),
        Some(p) if p == 443 && u.scheme() == "https" => Ok(format!("{}://{}", u.scheme(), host)),
        Some(p) => Ok(format!("{}://{}:{}", u.scheme(), host, p)),
        None => Ok(format!("{}://{}", u.scheme(), host)),
    }
}


pub(crate) fn credential_username(client: &GitaClient, owner: &str) -> Result<String> {
    if let Ok(Some(u)) = credential::get_username(&client.base_url) {
        return Ok(u);
    }
    Ok(owner.to_string())
}
