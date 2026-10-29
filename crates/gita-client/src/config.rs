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
    /// Resolution order for the server URL:
    /// `--url` / `GITA_URL` env > config file `server.url` > default
    /// `http://127.0.0.1:3000`.
    ///
    /// Token resolution order: `GITA_TOKEN` env > keyring > `None`.
    pub fn load(cli_url: Option<&str>) -> Result<Config> {
        let server_url = cli_url
            .map(|s| s.to_string())
            .or(std::env::var("GITA_URL").ok())
            .or_else(|| config_file_url())
            .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());

        let token = std::env::var("GITA_TOKEN")
            .ok()
            .or_else(|| credential::get_secret(&server_url).ok().flatten());

        Ok(Config { server_url, token })
    }

    /// Save the server URL to the config file.
    pub fn save_url(url: &str) -> Result<()> {
        let path = config_path();
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

fn config_path() -> PathBuf {
    if let Ok(home) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(home).join("gita/config.toml")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config/gita/config.toml")
    } else {
        PathBuf::from("config/gita.toml")
    }
}

fn config_file_url() -> Option<String> {
    let path = config_path();
    let content = std::fs::read_to_string(&path).ok()?;
    #[derive(Deserialize)]
    struct Cfg {
        server: Option<ServerCfg>,
    }
    #[derive(Deserialize)]
    struct ServerCfg {
        url: Option<String>,
    }
    let cfg: Cfg = toml::from_str(&content).ok()?;
    cfg.server.and_then(|s| s.url)
}
