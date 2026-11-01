//! Client configuration: server URL and token resolution.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::credential;

/// Client configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub server_url: String,
    pub token: Option<String>,
}

impl Config {
    /// Load configuration.
    ///
    /// Resolution order for the server URL: `--url` / `GITA_URL` env >
    /// config file `server.url`. When none is set this is an error rather
    /// than a silent default: sending credentials (or an implicit logout)
    /// to a guessed server is worse than failing fast with a hint.
    ///
    /// Token resolution order: `GITA_TOKEN` env > credential store (keyring
    /// or plaintext fallback) > `None`.
    pub fn load(cli_url: Option<&str>) -> Result<Config> {
        let server_url = cli_url
            .map(|s| s.to_string())
            .or(std::env::var("GITA_URL").ok())
            .or_else(|| config_file_url())
            .context("no server URL configured: pass --url, set GITA_URL, or run `gita config set-url <url>`")?;

        let token = std::env::var("GITA_TOKEN")
            .ok()
            .or_else(|| credential::get_secret(&server_url).ok().flatten());

        Ok(Config { server_url, token })
    }

    /// Save the server URL to the config file.
    pub fn save_url(url: &str) -> Result<()> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create config dir {}", parent.display()))?;
        }
        #[derive(Serialize)]
        struct Cfg {
            server: ServerCfg,
        }
        #[derive(Serialize)]
        struct ServerCfg {
            url: String,
        }
        let toml = toml::to_string_pretty(&Cfg {
            server: ServerCfg {
                url: url.to_string(),
            },
        })?;
        std::fs::write(&path, toml).with_context(|| format!("write config {}", path.display()))?;
        Ok(())
    }
}

/// Directory holding the client state: `config.toml` and the plaintext
/// credential fallback `credentials.toml`.
///
/// `$GITA_CONFIG_DIR` wins (this is also how the tests isolate themselves),
/// then `$HOME/.gita` — a single dot-directory next to `.gitconfig` is
/// easier to find and to wipe than an XDG tree.
pub fn config_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("GITA_CONFIG_DIR") {
        if !d.is_empty() {
            return Some(PathBuf::from(d));
        }
    }
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".gita"))
}

fn config_path() -> Result<PathBuf> {
    config_dir()
        .map(|d| d.join("config.toml"))
        .context("cannot determine the gita config directory (set HOME or GITA_CONFIG_DIR)")
}

/// The pre-`~/.gita` location. Read as a fallback so an existing setup
/// keeps working after the move; writes always go to the new path.
fn legacy_config_path() -> Option<PathBuf> {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return Some(PathBuf::from(x).join("gita/config.toml"));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".config/gita/config.toml"))
}

fn config_file_url() -> Option<String> {
    let path = match config_path() {
        Ok(p) => p,
        Err(_) => return None,
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            // New location absent — try the legacy one before giving up.
            let legacy = legacy_config_path()?;
            std::fs::read_to_string(legacy).ok()?
        }
    };
    parse_config_url(&content)
}

fn parse_config_url(content: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Cfg {
        server: Option<ServerCfg>,
    }
    #[derive(Deserialize)]
    struct ServerCfg {
        url: Option<String>,
    }
    let cfg: Cfg = toml::from_str(content).ok()?;
    cfg.server.and_then(|s| s.url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_url_is_parsed_from_the_toml() {
        assert_eq!(
            parse_config_url("[server]\nurl = \"http://h:1\""),
            Some("http://h:1".to_string())
        );
        assert_eq!(parse_config_url(""), None);
        assert_eq!(parse_config_url("[server]\n"), None);
    }
}
