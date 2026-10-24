//! Authentication flows and credential storage helpers.
//!
//! Token resolution order (in `Config::load`): `GITA_TOKEN` env > keyring >
//! `None`. Commands below that need a token therefore work either from the
//! env var or from a previously stored keyring secret.

use anyhow::{Context, Result};
use std::io::Write;

use crate::api::GitaClient;
use crate::credential;
use crate::output;

/// Dispatch an `auth` subcommand.
pub async fn dispatch(
    cmd: crate::args::AuthCommand,
    client: &GitaClient,
    json: bool,
) -> Result<()> {
    match cmd {
        crate::args::AuthCommand::Login { url } => login(client, url.as_deref(), json).await,
        crate::args::AuthCommand::Register {
            username,
            email,
            password,
        } => {
            register(
                client,
                username.as_deref(),
                email.as_deref(),
                password.as_deref(),
                json,
            )
            .await
        }
        crate::args::AuthCommand::Logout => logout(client, json).await,
        crate::args::AuthCommand::Status => status(client, json).await,
        crate::args::AuthCommand::Whoami => whoami(client, json).await,
        crate::args::AuthCommand::Token { cmd } => match cmd {
            crate::args::TokenCommand::Create { name, store } => {
                token_create(client, name.as_deref(), store, json).await
            }
            crate::args::TokenCommand::List => token_list(client, json).await,
            crate::args::TokenCommand::Delete { id } => token_delete(client, id, json).await,
        },
    }
}

/// Log in: prompt for username/password, POST /api/auth/login, store token.
async fn login(client: &GitaClient, _url: Option<&str>, json: bool) -> Result<()> {
    let username = prompt("username: ")?;
    let password = prompt_password("password: ")?;
    let auth = client.login(&username, &password).await?;
    // Keyring is best-effort: in CI/headless there may be no Secret Service,
    // but the token was still obtained from the server.
    if let Err(e) = credential::set_secret(&client.base_url, &auth.token) {
        eprintln!("warning: could not store token in keyring: {e}");
    }
    if let Err(e) = credential::set_username(&client.base_url, &auth.user.username) {
        eprintln!("warning: could not store username in keyring: {e}");
    }
    if json {
        output::print_json(&auth)?;
    } else {
        println!("Logged in as {}", auth.user.username);
    }
    Ok(())
}

/// Register a new account, store token.
async fn register(
    client: &GitaClient,
    username: Option<&str>,
    email: Option<&str>,
    password: Option<&str>,
    json: bool,
) -> Result<()> {
    let username = username
        .map(|s| s.to_string())
        .or_else(|| prompt("username: ").ok())
        .context("username required")?;
    let email = email
        .map(|s| s.to_string())
        .or_else(|| prompt("email: ").ok())
        .context("email required")?;
    let password = password
        .map(|s| s.to_string())
        .or_else(|| prompt_password("password: ").ok())
        .context("password required")?;
    let auth = client.register(&username, &email, &password).await?;
    // Keyring is best-effort: in CI/headless there may be no Secret Service,
    // but the token was still obtained from the server.
    if let Err(e) = credential::set_secret(&client.base_url, &auth.token) {
        eprintln!("warning: could not store token in keyring: {e}");
    }
    if let Err(e) = credential::set_username(&client.base_url, &auth.user.username) {
        eprintln!("warning: could not store username in keyring: {e}");
    }
    if json {
        output::print_json(&auth)?;
    } else {
        println!("Registered and logged in as {}", auth.user.username);
    }
    Ok(())
}

/// Log out: revoke the session server-side, forget the local token.
async fn logout(client: &GitaClient, _json: bool) -> Result<()> {
    match client.logout().await {
        Ok(msg) => println!("{}", msg.message),
        Err(e) => {
            // Logout with a PAT returns a message rather than an error;
            // surface it and still clear the local copy.
            eprintln!("{e:#}");
        }
    }
    if let Err(e) = credential::delete_secret(&client.base_url) {
        eprintln!("warning: could not clear keyring entry: {e}");
    }
    println!("Logged out.");
    Ok(())
}

/// Show sign-in status: resolve token (env or keyring), validate with /api/auth/me.
async fn status(client: &GitaClient, json: bool) -> Result<()> {
    if client.token.is_none() {
        if json {
            output::print_json(&serde_json::json!({ "logged_in": false }))?;
        } else {
            println!("Not logged in to {}", client.base_url);
        }
        return Ok(());
    }
    match client.me().await {
        Ok(user) => {
            if json {
                output::print_json(&serde_json::json!({
                    "logged_in": true,
                    "server": client.base_url,
                    "user": user,
                }))?;
            } else {
                println!(
                    "Logged in to {} as {} ({})",
                    client.base_url, user.username, user.email
                );
            }
        }
        Err(e) => {
            if json {
                output::print_json(
                    &serde_json::json!({ "logged_in": false, "error": e.to_string() }),
                )?;
            } else {
                println!("Token present but invalid for {}: {}", client.base_url, e);
            }
        }
    }
    Ok(())
}

/// Show the authenticated user.
async fn whoami(client: &GitaClient, json: bool) -> Result<()> {
    let user = client.me().await?;
    if json {
        output::print_json(&user)?;
    } else {
        println!("username: {}", user.username);
        println!("email: {}", user.email);
        println!("locale: {}", user.locale);
    }
    Ok(())
}

/// Create a personal access token (plaintext shown once).
async fn token_create(
    client: &GitaClient,
    name: Option<&str>,
    store: bool,
    json: bool,
) -> Result<()> {
    let token = client.create_token(name).await?;
    if json {
        output::print_json(&token)?;
    } else {
        println!("Token created — shown only once:");
        println!("  id: {}", token.id);
        println!("  name: {}", token.name);
        println!("  token: {}", token.token);
        println!("Save it somewhere safe — it will not be shown again.");
    }
    if store {
        if let Err(e) = credential::set_secret(&client.base_url, &token.token) {
            eprintln!("warning: could not store token in keyring: {e}");
        } else {
            println!("Stored as the active token for {}", client.base_url);
        }
    }
    Ok(())
}

/// List personal access tokens (prefixes only).
async fn token_list(client: &GitaClient, json: bool) -> Result<()> {
    let tokens = client.list_tokens().await?;
    if json {
        output::print_json(&tokens)?;
    } else {
        let mut table = output::Table::new(&["id", "prefix", "name", "created_at"]);
        for t in &tokens {
            table.add_row(vec![
                t.id.to_string(),
                t.prefix.as_deref().unwrap_or("").to_string(),
                t.name.clone(),
                t.created_at.clone().unwrap_or_default(),
            ]);
        }
        table.print();
    }
    Ok(())
}

/// Delete a personal access token.
async fn token_delete(client: &GitaClient, id: i64, _json: bool) -> Result<()> {
    client.delete_token(id).await?;
    println!("Token {id} deleted.");
    Ok(())
}

/// Prompt for a line on stdin (no echo).
fn prompt(msg: &str) -> Result<String> {
    print!("{msg}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

/// Prompt for a password without echo.
// Prefers the `GITA_PASSWORD` env var (scripting). Otherwise reads a line
// from stdin with echo on — acceptable for interactive use on a trusted
// terminal; for no-echo, pipe the password via `GITA_PASSWORD` or stdin.
fn prompt_password(msg: &str) -> Result<String> {
    if let Ok(pw) = std::env::var("GITA_PASSWORD") {
        return Ok(pw);
    }
    print!("{msg}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}
