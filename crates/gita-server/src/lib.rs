//! Gita server: configuration, application assembly and startup.

use anyhow::{Context, Result};
use clap::Parser;
use gita_ai::{AiEngine, LlmConfig};
use gita_api::auth::Auth;
use gita_api::search::{Fts5Backend, SearchService};
use gita_db::Db;
use gita_events::EventBus;
use gita_git::GitManager;
use gita_http::GitHttpState;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

/// Command-line arguments.
#[derive(Debug, Parser)]
#[command(name = "gita", version, about = "AI-native Git server")]
pub struct Args {
    /// Path to the configuration file.
    #[arg(short, long, default_value = "config/gita.toml")]
    pub config: PathBuf,
}

/// Server configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub server: ServerConfig,
    pub llm: LlmConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default = "default_db_url")]
    pub database_url: String,
    #[serde(default = "default_jwt_secret")]
    pub jwt_secret: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            data_dir: default_data_dir(),
            database_url: default_db_url(),
            jwt_secret: default_jwt_secret(),
        }
    }
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_port() -> u16 {
    3000
}
fn default_data_dir() -> PathBuf {
    PathBuf::from("data")
}
fn default_db_url() -> String {
    "sqlite://data/gita.db".to_string()
}
fn default_jwt_secret() -> String {
    "dev-secret-change-me".to_string()
}

impl Config {
    /// Load config from a TOML file, merging defaults.
    pub fn load(path: &PathBuf) -> Result<Self> {
        let mut config = Config::default();
        if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("read config file {}", path.display()))?;
            let parsed: Config = toml::from_str(&text)
                .with_context(|| format!("parse config file {}", path.display()))?;
            // Merge fields.
            if !parsed.server.host.is_empty() {
                config.server.host = parsed.server.host;
            }
            if parsed.server.port != 0 {
                config.server.port = parsed.server.port;
            }
            if !parsed.server.data_dir.as_os_str().is_empty() {
                config.server.data_dir = parsed.server.data_dir;
            }
            if !parsed.server.database_url.is_empty() {
                config.server.database_url = parsed.server.database_url;
            }
            if !parsed.server.jwt_secret.is_empty() && parsed.server.jwt_secret != default_jwt_secret() {
                config.server.jwt_secret = parsed.server.jwt_secret;
            }
            if parsed.llm.base_url.is_some() || parsed.llm.api_key.is_some() {
                config.llm = parsed.llm;
            }
        } else {
            warn!(
                "config file {} not found, using defaults",
                path.display()
            );
        }
        Ok(config)
    }
}

/// The assembled application state.
#[derive(Clone)]
pub struct App {
    pub db: Db,
    pub git: Arc<GitManager>,
    pub bus: EventBus,
    pub api_state: gita_api::AppState,
    pub git_http_state: GitHttpState,
    pub config: Config,
}

impl App {
    /// Build the application from configuration.
    pub async fn build(config: Config) -> Result<Self> {
        // Prepare data directories.
        std::fs::create_dir_all(&config.server.data_dir)
            .with_context(|| format!("create data dir {}", config.server.data_dir.display()))?;
        let repos_dir = config.server.data_dir.join("repos");
        std::fs::create_dir_all(&repos_dir)
            .with_context(|| format!("create repos dir {}", repos_dir.display()))?;

        // Resolve database URL relative to data_dir if it's a relative SQLite path.
        let mut db_url = config.server.database_url.clone();
        if let Some(path) = db_url.strip_prefix("sqlite://") {
            if !path.starts_with('/') && !path.starts_with(':') {
                let p = config.server.data_dir.join(path);
                db_url = format!("sqlite://{}", p.display());
            }
        }

        info!("connecting to database");
        let db = Db::connect(&db_url)
            .await
            .with_context(|| format!("connect to database {db_url}"))?;

        let git = Arc::new(GitManager::new(repos_dir));
        let bus = EventBus::default();
        let auth = Auth::new(config.server.jwt_secret.clone());
        let ai = AiEngine::from_config(config.llm.clone());
        if !ai.configured {
            warn!("no LLM API key configured — AI reviews will use the stub provider");
        }

        let search_backend = Arc::new(Fts5Backend {
            db: db.clone(),
            git: git.clone(),
        });
        let search = SearchService::new(search_backend);

        let api_state = gita_api::AppState::new(
            db.clone(),
            auth,
            (*git).clone(),
            bus.clone(),
            ai.clone(),
            search.clone(),
            config.llm.clone(),
        );

        let git_http_state = GitHttpState {
            git: git.clone(),
            db: db.clone(),
            bus: bus.clone(),
        };

        Ok(Self {
            db,
            git,
            bus,
            api_state,
            git_http_state,
            config,
        })
    }

    /// Build the full axum router.
    pub fn router(&self) -> axum::Router {
        let api_router = gita_api::api_router(self.api_state.clone());
        let git_router = gita_http::router(self.git_http_state.clone());
        let web_router = gita_web::web_router(Some(self.config.server.data_dir.join("web")));

        axum::Router::new()
            .merge(api_router)
            .merge(git_router)
            .merge(web_router)
    }
}

/// Run the server.
pub async fn run(config: Config) -> Result<()> {
    let app = App::build(config.clone()).await?;
    let router = app.router();

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    info!("Gita listening on http://{addr}");

    axum::serve(listener, router).await.context("axum serve error")
}