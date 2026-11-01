//! Authentication flows and credential storage helpers.
//!
//! Token resolution order (in `Config::load`): `GITA_TOKEN` env > keyring >
//! `None`. Commands below that need a token therefore work either from the
//! env var or from a previously stored keyring secret.

use anyhow::{bail, Context, Result};
use std::io::Write;

use gita_client::api::GitaClient;
use gita_client::credential;
use gita_client::output;

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
        crate::args::AuthCommand::Me {
            locale,
            email,
            current_password,
            new_password,
        } => {
            me_update(
                client,
                locale.as_deref(),
                email.as_deref(),
                current_password.as_deref(),
                new_password.as_deref(),
                json,
            )
            .await
        }
        crate::args::AuthCommand::Token { cmd } => match cmd {
            crate::args::TokenCommand::Create { name, store } => {
                token_create(client, name.as_deref(), store, json).await
            }
            crate::args::TokenCommand::List => token_list(client, json).await,
            crate::args::TokenCommand::Delete { id } => token_delete(client, id, json).await,
        },
        crate::args::AuthCommand::Credential { operation } => {
            credential_helper(&operation)
        }
    }
}

/// A git credential-helper request.
#[derive(Debug, Default)]
struct CredentialRequest {
    protocol: Option<String>,
    host: Option<String>,
    path: Option<String>,
    username: Option<String>,
    password: Option<String>,
}

impl CredentialRequest {
    /// Read git's `key=value` request format from stdin.
    fn read() -> Result<Self> {
        let mut req = CredentialRequest::default();
        let mut line = String::new();
        loop {
            line.clear();
            let n = std::io::stdin().read_line(&mut line)?;
            if n == 0 {
                break;
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            let Some((key, value)) = trimmed.split_once('=') else {
                continue;
            };
            match key {
                "protocol" => req.protocol = Some(value.to_string()),
                "host" => req.host = Some(value.to_string()),
                "path" => req.path = Some(value.to_string()),
                "username" => req.username = Some(value.to_string()),
                "password" => req.password = Some(value.to_string()),
                _ => {}
            }
        }
        Ok(req)
    }

    /// The server URL this request refers to, without the repository path.
    fn server_url(&self) -> Option<String> {
        let protocol = self.protocol.as_deref()?;
        let host = self.host.as_deref()?;
        Some(format!("{protocol}://{host}"))
    }
}

/// Serve git's credential helper protocol.
///
/// Git calls the helper as `git credential <op>` with a `key=value` request
/// on stdin; `get` must print `username=`/`password=` lines, `store` and
/// `erase` take the same request shape. The point of the helper is that
/// the token the user already logged in with is reused for HTTPS pushes,
/// so nobody has to paste a token into git's prompt again.
pub(crate) fn credential_helper(operation: &str) -> Result<()> {
    let req = CredentialRequest::read()?;
    match operation {
        "get" => {
            let (username, token) = lookup_credential(&req);
            match (username, token) {
                (Some(username), Some(token)) => {
                    println!("username={username}");
                    println!("password={token}");
                }
                // Nothing stored: exit cleanly with no output, which git
                // treats as "no credential here" and answers with its own
                // prompt.
                _ => {}
            }
            Ok(())
        }
        "store" => {
            // Only keep credentials for servers we already know. The
            // helper is registered for git as a whole, so `git push
            // origin` (GitHub, say) would otherwise end up writing that
            // host's password into our plaintext file.
            let Some(url) = req.server_url() else {
                return Ok(());
            };
            if !is_known_server(&url) {
                return Ok(());
            }
            if let Some(username) = req.username.as_deref() {
                let _ = credential::set_username(&url, username);
            }
            if let Some(password) = req.password.as_deref() {
                let _ = credential::set_secret(&url, password);
            }
            Ok(())
        }
        "erase" => {
            // Git asks to erase the credential it holds — that pair, not
            // "everything for this host". Wiping the whole entry would
            // throw away a good token whenever git erases a stale one it
            // happened to try first.
            let Some(url) = req.server_url() else {
                return Ok(());
            };
            match req.password.as_deref() {
                Some(password) => {
                    let stored = credential::get_secret(&url).ok().flatten();
                    if stored.as_deref() == Some(password) {
                        let _ = credential::delete_secret(&url);
                    }
                }
                // No password in the request: git wants the host's
                // credentials gone.
                None => {
                    let _ = credential::delete_secret(&url);
                }
            }
            Ok(())
        }
        other => bail!("unknown credential operation {other}; expected get, store or erase"),
    }
}

/// Find the username and token to hand to git for `req`.
///
/// First the file fallback, matched by scheme and host. Then the default
/// server's own entry — the keyring has no "list" operation, so a token
/// that lives only in the keyring is reachable through the default URL.
///
/// The default server is only consulted when git is actually asking
/// about *that* server: handing a gita token to git for some other host
/// (github.com, say) would ship the secret to a third party.
fn lookup_credential(req: &CredentialRequest) -> (Option<String>, Option<String>) {
    if let (Some(protocol), Some(host)) = (req.protocol.as_deref(), req.host.as_deref()) {
        if let Ok(Some(site)) = credential::lookup_by_host(protocol, host) {
            if site.token.is_some() {
                return (site.username, site.token);
            }
        }
    }
    // Fall back to the default server — but only for that server.
    let Some(request_url) = req.server_url() else {
        return (None, None);
    };
    if let Ok(config) = gita_client::config::Config::load(None) {
        if credential::same_host(&config.server_url, &request_url) {
            let token = credential::get_secret(&config.server_url).ok().flatten();
            let username = credential::get_username(&config.server_url).ok().flatten();
            if token.is_some() {
                return (username, token);
            }
        }
    }
    (None, None)
}

/// Whether `url` is a server this CLI already holds credentials for.
fn is_known_server(url: &str) -> bool {
    let Some((scheme, host)) = credential::split_scheme_host(url) else {
        return false;
    };
    if let Ok(Some(site)) = credential::lookup_by_host(&scheme, &host) {
        if site.token.is_some() {
            return true;
        }
    }
    if let Ok(config) = gita_client::config::Config::load(None) {
        return credential::same_host(&config.server_url, url);
    }
    false
}

/// Store the token for `base_url`, telling the user where it landed.
///
/// The keyring is best-effort: when it is unavailable the token degrades
/// to the plaintext credentials file, which the user must know about —
/// and if neither store works, later commands need `GITA_TOKEN`, so that
/// must be said too.
fn store_token(base_url: &str, token: &str) {
    match credential::set_secret(base_url, token) {
        Ok(credential::Store::Keyring) => {}
        Ok(credential::Store::PlaintextFile) => {
            let path = credential::credentials_file()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "~/.gita/credentials.toml".to_string());
            eprintln!("warning: no usable OS keyring on this machine — the token is stored in plaintext at {path} (mode 0600)");
            eprintln!("hint: a Secret Service (gnome-keyring, kwallet) would keep it in the keyring instead");
        }
        Err(e) => eprintln!(
            "warning: the token could not be stored ({e}); export GITA_TOKEN for subsequent commands"
        ),
    }
}

/// Remember the server just logged into as the default.
///
/// Credentials are keyed by server URL, so a login against `--url` that is
/// not the configured default would leave later commands (which resolve
/// the default) unable to find the token. `gh auth login` behaves the same
/// way. Failures are non-fatal: the session itself is already stored.
fn make_default_server(client: &GitaClient, json: bool) {
    if let Err(e) = gita_client::config::Config::save_url(&client.base_url) {
        eprintln!("warning: could not save the default server URL: {e}");
    } else if !json {
        println!("Default server set to {}", client.base_url);
    }
}

/// Log in: prompt for username/password, POST /api/auth/login, store token.
async fn login(client: &GitaClient, _url: Option<&str>, json: bool) -> Result<()> {
    let username = prompt("username: ")?;
    let password = prompt_password("password: ")?;
    let auth = client.login(&username, &password).await?;
    store_token(&client.base_url, &auth.token);
    if let Err(e) = credential::set_username(&client.base_url, &auth.user.username) {
        eprintln!("warning: could not store username: {e}");
    }
    make_default_server(client, json);
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
    store_token(&client.base_url, &auth.token);
    if let Err(e) = credential::set_username(&client.base_url, &auth.user.username) {
        eprintln!("warning: could not store username: {e}");
    }
    make_default_server(client, json);
    if json {
        output::print_json(&auth)?;
    } else {
        println!("Registered and logged in as {}", auth.user.username);
    }
    Ok(())
}

/// Log out: revoke the session server-side, forget the local token.
async fn logout(client: &GitaClient, _json: bool) -> Result<()> {
    // The server answers with its own message ("logged out"); printing
    // ours as well would say the same thing twice.
    let revoked = match client.logout().await {
        Ok(msg) => {
            let message = msg.message.trim();
            if !message.is_empty() {
                println!("{message}");
            }
            true
        }
        Err(e) => {
            // Logout with a PAT returns a message rather than an error;
            // surface it and still clear the local copy.
            eprintln!("{e:#}");
            false
        }
    };
    if let Err(e) = credential::delete_secret(&client.base_url) {
        eprintln!("warning: could not clear the stored credentials: {e}");
    }
    if !revoked {
        println!("Logged out locally; the server session was already gone.");
    }
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

/// Update the authenticated user's profile (locale, email, password).
async fn me_update(
    client: &GitaClient,
    locale: Option<&str>,
    email: Option<&str>,
    current_password: Option<&str>,
    new_password: Option<&str>,
    json: bool,
) -> Result<()> {
    if locale.is_none() && email.is_none() && new_password.is_none() {
        bail!("nothing to update; pass --locale, --email and/or --new-password");
    }
    if new_password.is_some() && current_password.is_none() {
        bail!("--current-password is required when changing the password");
    }
    let user = client
        .update_me(locale, email, current_password, new_password)
        .await?;
    if json {
        output::print_json(&user)?;
    } else {
        println!("Updated profile:");
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
        store_token(&client.base_url, &token.token);
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
/// Prefers the `GITA_PASSWORD` env var (scripting). Uses rpassword on TTY;
/// falls back to reading stdin (with echo) when not on a TTY (e.g. piped input).
fn prompt_password(msg: &str) -> Result<String> {
    if let Ok(pw) = std::env::var("GITA_PASSWORD") {
        return Ok(pw);
    }
    print!("{msg}");
    std::io::stdout().flush()?;
    use std::io::{stdin, IsTerminal};
    if stdin().is_terminal() {
        Ok(rpassword::read_password()?)
    } else {
        let mut line = String::new();
        stdin().read_line(&mut line)?;
        Ok(line.trim().to_string())
    }
}
