//! Gita server: configuration, application assembly and startup.

use anyhow::{Context, Result};
use clap::Parser;
use gita_api::auth::Auth;
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

/// Server configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub server: ServerConfig,
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
    /// Optional Postgres schema to pin `search_path` to. Used by tests for
    /// per-test schema isolation.
    #[serde(default)]
    pub database_schema: Option<String>,
    /// Embedded SSH server configuration (Git over SSH).
    #[serde(default)]
    pub ssh: SshConfig,
    /// Master switch for GitHub-Pages-style site serving. When false, the
    /// `/{owner}/{repo}/{*path}` routes are not registered at all and every
    /// request falls through to the SPA.
    #[serde(default = "default_pages_enabled")]
    pub pages_enabled: bool,
    /// Optional URL prefix for Pages sites, e.g. "/p" serves /p/owner/repo/.
    /// Empty (the default) serves at /owner/repo/, mirroring GitHub project
    /// sites.
    #[serde(default)]
    pub pages_url_prefix: String,
    /// Return the password-reset token in the API response.
    ///
    /// Off by default: the response is uniform for existing and
    /// non-existent accounts, so returning the token would let an
    /// unauthenticated caller enumerate accounts *and* take them over. Turn
    /// it on only for local development or a deployment with no mail
    /// transport, where an operator reads the token out of the response.
    #[serde(default)]
    pub allow_insecure_password_reset: bool,
}

/// Configuration for the embedded SSH server (Git over SSH).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConfig {
    /// Whether to start the SSH listener alongside HTTP.
    #[serde(default = "default_ssh_enabled")]
    pub enabled: bool,
    /// Bind address for the SSH listener.
    #[serde(default = "default_ssh_host")]
    pub host: String,
    /// Port for the SSH listener.
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// Path to the OpenSSH-format host private key. Defaults to
    /// `<data_dir>/ssh_host_ed25519_key` and is auto-generated on first run.
    #[serde(default)]
    pub host_key: Option<PathBuf>,
}

fn default_ssh_enabled() -> bool {
    false
}
fn default_ssh_host() -> String {
    default_host()
}
fn default_ssh_port() -> u16 {
    2222
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            enabled: default_ssh_enabled(),
            host: default_ssh_host(),
            port: default_ssh_port(),
            host_key: None,
        }
    }
}

fn default_pages_enabled() -> bool {
    true
}

/// Per-IP registration rate limit: one request per second, with a burst of
/// three.
///
/// Registration is the expensive endpoint — every call runs an Argon2 hash
/// and creates a row — so it gets a tighter quota than login (5/s, burst
/// 10). The burst of three lets a user who mistyped their username retry
/// immediately without waiting.
const REGISTER_RATE_PER_SECOND: u32 = 1;
const REGISTER_RATE_BURST: u32 = 3;

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            data_dir: default_data_dir(),
            database_url: default_db_url(),
            jwt_secret: default_jwt_secret(),
            database_schema: None,
            ssh: SshConfig::default(),
            pages_enabled: default_pages_enabled(),
            pages_url_prefix: String::new(),
            allow_insecure_password_reset: false,
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
    "postgres://gita:gita@localhost:5432/gita".to_string()
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
            // SSH section (only when present in the file).
            // NOTE: this pattern is asymmetric — a boolean can only ever be
            // turned *on* by the file, never off. Pages uses presence-based
            // merging below instead; keep this block as-is to avoid churn.
            if parsed.server.ssh.enabled {
                config.server.ssh.enabled = true;
            }
            if !parsed.server.ssh.host.is_empty() {
                config.server.ssh.host = parsed.server.ssh.host;
            }
            if parsed.server.ssh.port != 0 {
                config.server.ssh.port = parsed.server.ssh.port;
            }
            if parsed.server.ssh.host_key.is_some() {
                config.server.ssh.host_key = parsed.server.ssh.host_key;
            }

            // Pages section. Unlike SSH, these fields must be switchable in
            // both directions, so check key *presence* in the raw TOML rather
            // than the defaulted parsed struct (which cannot distinguish
            // "absent" from "explicitly false"/"empty").
            let raw: toml::Value = match toml::from_str(&text) {
                Ok(v) => v,
                // The same text already parsed successfully above, so this
                // can only fail if the file changed between reads.
                Err(_) => toml::Value::Table(Default::default()),
            };
            let raw_server = raw.get("server");
            if let Some(v) = raw_server
                .and_then(|s| s.get("pages_enabled"))
                .and_then(|v| v.as_bool())
            {
                config.server.pages_enabled = v;
            }
            if let Some(v) = raw_server
                .and_then(|s| s.get("pages_url_prefix"))
                .and_then(|v| v.as_str())
            {
                config.server.pages_url_prefix = v.to_string();
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
    /// The data directory (config.server.data_dir). Mirrors
    /// `config.server.data_dir` for convenient access from integration
    /// tests that need to push real git commits into the repository
    /// before exercising API endpoints that read git history.
    pub data_dir: std::path::PathBuf,
    /// Running SSH server (Git over SSH), when enabled in config.
    pub ssh_handle: Option<Arc<gita_ssh::SshServerHandle>>,
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

        info!("connecting to database");
        let db = match &config.server.database_schema {
            Some(schema) => Db::connect_with_schema(&config.server.database_url, schema)
                .await
                .with_context(|| {
                    format!(
                        "connect to database {} (schema {})",
                        config.server.database_url, schema
                    )
                })?,
            None => Db::connect(&config.server.database_url)
                .await
                .with_context(|| {
                    format!("connect to database {}", config.server.database_url)
                })?,
        };

        let git = Arc::new(GitManager::new(repos_dir));
        // Persist every published event to the database so the `/api/events`
        // endpoint and AI-readable event stream reflect real activity.
        // Also fan out repository-scoped events to any registered webhooks.
        let event_db = db.clone();
        let event_git_root = git.root().to_path_buf();
        let bus = EventBus::new(Some(Arc::new(move |event: gita_core::Event| {
            let db = event_db.clone();
            let git_root = event_git_root.clone();
            tokio::spawn(async move {
                if let Err(e) = db.insert_event(&event).await {
                    warn!("failed to persist event: {e}");
                }
                // Webhook fan-out: only when the event has a repo scope.
                if let Some(repo_id) = event.repo_id {
                    match db.list_webhooks(repo_id).await {
                        Ok(hooks) => {
                            for hook in hooks {
                                if !hook.active {
                                    continue;
                                }
                                if !hook.events.is_empty()
                                    && !hook.events
                                        .iter()
                                        .any(|e| e == &event.event_type.to_string())
                                {
                                    continue;
                                }
                                let url = hook.url.clone();
                                let hook_id = hook.id;
                                let secret = hook.secret.clone();
                                let event_name = event.event_type.to_string();
                                let payload = serde_json::json!({
                                    "event": event_name,
                                    "actor_id": event.actor_id,
                                    "repo_id": event.repo_id,
                                    "payload": event.payload,
                                    "created_at": event.created_at.to_rfc3339(),
                                });
                                let body = serde_json::to_string(&payload).unwrap_or_default();
                                tokio::spawn(async move {
                                    let client = reqwest::Client::new();
                                    let mut req = client
                                        .post(&url)
                                        .header("content-type", "application/json")
                                        .header("x-gita-event", &event_name);
                                    // Sign the body with the hook's secret so
                                    // the receiver can verify it came from
                                    // us. A hook without a secret is still
                                    // delivered, but unsigned.
                                    if !secret.is_empty() {
                                        let signature = sign_webhook_payload(&secret, &body);
                                        req = req.header(
                                            "x-gita-signature-256",
                                            format!("sha256={signature}"),
                                        );
                                    }
                                    let req = req.body(body);
                                    match req.send().await {
                                        Ok(_) => {
                                            tracing::info!("webhook {hook_id} delivered to {url}")
                                        }
                                        Err(e) => {
                                            tracing::warn!("webhook {hook_id} to {url} failed: {e}")
                                        }
                                    }
                                    let _ = git_root;
                                });
                            }
                        }
                        Err(e) => {
                            tracing::warn!("failed to load webhooks for repo {repo_id}: {e}");
                        }
                    }
                }
            });
        })));
        let auth = Auth::new(config.server.jwt_secret.clone());

        let api_state = gita_api::AppState::new(
            db.clone(),
            auth.clone(),
            (*git).clone(),
            bus.clone(),
        )
        .with_avatar_dir(config.server.data_dir.join("avatars"));

        let git_http_state = GitHttpState {
            git: git.clone(),
            db: db.clone(),
            bus: bus.clone(),
            auth: Arc::new(auth.clone()),
        };

        // Ensure the SSH host key exists (generated on first run) so a
        // later `run()` can start the SSH listener with a stable identity.
        if config.server.ssh.enabled {
            let host_key_path = config
                .server
                .ssh
                .host_key
                .clone()
                .unwrap_or_else(|| config.server.data_dir.join(gita_ssh::DEFAULT_HOST_KEY_NAME));
            gita_ssh::load_or_create_host_key(&host_key_path)?;
        }

        Ok(Self {
            db,
            git,
            bus,
            api_state,
            git_http_state,
            data_dir: config.server.data_dir.clone(),
            config,
            ssh_handle: None,
        })
    }

    /// Build the full axum router.
    pub fn router(&self) -> axum::Router {
        self.router_with_limiter(gita_api::build_limiter(100, 200))
    }

    /// Build the full axum router with an explicit per-IP login limiter.
    ///
    /// The registration limiter is the default one; use
    /// [`App::router_with_limiters`] to control both.
    pub fn router_with_limiter(&self, limiter: gita_api::SharedLimiter) -> axum::Router {
        self.router_with_limiters(
            limiter,
            gita_api::build_limiter(REGISTER_RATE_PER_SECOND, REGISTER_RATE_BURST),
        )
    }

    /// Build the full axum router with explicit per-IP limiters for both
    /// login and registration.
    ///
    /// The two get separate buckets on purpose. Registration is the more
    /// expensive endpoint (every call runs an Argon2 hash) and it creates a
    /// new account per call, so a script could otherwise fill the users
    /// table at full request rate — while brute-forcing a *known* account
    /// was limited. Sharing one bucket would also let login traffic starve
    /// registration (and the reverse).
    pub fn router_with_limiters(
        &self,
        login_limiter: gita_api::SharedLimiter,
        register_limiter: gita_api::SharedLimiter,
    ) -> axum::Router {
        // Install the limiters into AppState so the handlers can check the
        // buckets; the rest of the API is unaffected.
        let api_state = self
            .api_state
            .clone()
            .with_login_limiter(login_limiter)
            .with_register_limiter(register_limiter);
        let api_router = gita_api::api_router(api_state.clone());
        let git_router = gita_http::router(self.git_http_state.clone());
        // Serve the built React frontend (`web/dist`) when present.
        let web_router = match find_web_dist() {
            Some(dist) => gita_web::web_router(Some(dist)),
            None => {
                warn!("web/dist not found; serving fallback landing page");
                gita_web::web_router(None)
            }
        };
        // Pages static sites, mounted at the server root. When disabled by
        // config, skip registration entirely so every Pages-shaped URL falls
        // through to the SPA.
        let pages_router = if self.config.server.pages_enabled {
            let opts = gita_api::PagesOptions {
                url_prefix: self.config.server.pages_url_prefix.clone(),
            };
            gita_api::pages_router(api_state, opts)
        } else {
            axum::Router::new()
        };

        // Merge order does not matter: axum tries the path router before any
        // fallback service, so Pages' explicit routes always win over the SPA
        // fallback. Do not "reorder" this for readability.
        axum::Router::new()
            .merge(api_router)
            .merge(git_router)
            .merge(pages_router)
            .merge(web_router)
    }
}

/// Locate the built React frontend (`web/dist`).
///
/// The frontend may live in a few different places depending on how the
/// binary was built and deployed:
/// 1. `<CARGO_MANIFEST_DIR>/../../web/dist` — building from the workspace
///    source tree (compile-time absolute path).
/// 2. `<exe_dir>/web/dist` — a container/image layout where the binary and
///    frontend are siblings under a common root.
/// 3. `<current_dir>/web/dist` — running from the workspace root.
fn find_web_dist() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from));
    let cwd = std::env::current_dir().ok();

    let candidates = [
        manifest.join("../../web/dist"),
        exe_dir
            .as_ref()
            .map(|d| d.join("web/dist"))
            .unwrap_or_default(),
        cwd.as_ref().map(|d| d.join("web/dist")).unwrap_or_default(),
    ];

    candidates
        .into_iter()
        .find(|p| p.join("index.html").is_file())
}

/// Run the server.
pub async fn run(config: Config) -> Result<()> {
    let mut app = App::build(config.clone()).await?;

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    info!("Gita listening on http://{addr}");

    // Start the SSH listener (Git over SSH) when enabled.
    if config.server.ssh.enabled {
        let ssh_cfg = gita_ssh::SshServerConfig {
            listen_addr: format!("{}:{}", config.server.ssh.host, config.server.ssh.port),
            host_key: gita_ssh::load_or_create_host_key(
                &config
                    .server
                    .ssh
                    .host_key
                    .clone()
                    .unwrap_or_else(|| config.server.data_dir.join(gita_ssh::DEFAULT_HOST_KEY_NAME)),
            )?,
            repos_root: config.server.data_dir.join("repos"),
        };
        let handle = gita_ssh::start(ssh_cfg, app.db.clone()).await?;
        info!(
            "Gita SSH listening on {} (git@{}:{}/owner/repo.git)",
            handle.addr(),
            config.server.ssh.host,
            config.server.ssh.port
        );
        app.ssh_handle = Some(Arc::new(handle));
    }

    // Per-IP login rate limiter: 5 req/s with burst 10 — slows brute force
    // without affecting normal users.
    let limiter = gita_api::build_limiter(5, 10);
    let router = app.router_with_limiter(limiter);    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .context("axum serve error")
}

/// Mint a runner token for `username`, stored under `name`, and return
/// the raw token for the caller to print.
pub async fn worker_register(config: &Config, name: &str, username: &str) -> Result<String> {
    let db = Db::connect(&config.server.database_url).await?;
    let user = db
        .get_user_by_username(username)
        .await?
        .ok_or_else(|| anyhow::anyhow!("user not found: {username}"))?;

    // 32 random bytes hex-encoded — same shape as the REST PAT flow.
    let token = random_hex_token();
    db.create_access_token(user.id, &token, &format!("runner/{name}"))
        .await?;
    Ok(token)
}

/// Sign a webhook payload with the hook's secret.
///
/// GitHub's model: the receiver recomputes HMAC-SHA256 over the raw request
/// body with the shared secret and compares it to the signature the sender
/// put in the header. A mismatch means the payload was tampered with or did
/// not come from this server.
///
/// Returns a lowercase hex digest.
pub fn sign_webhook_payload(secret: &str, body: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts keys of any length");
    mac.update(body.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Generate a random hex token for runner registration.
fn random_hex_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_load_missing_file_falls_back_to_defaults() {
        let missing = PathBuf::from("/nonexistent/gita.toml");
        let config = Config::load(&missing).unwrap();
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.jwt_secret, "dev-secret-change-me");
        assert!(config.server.database_url.contains("postgres://"));
    }

    #[test]
    fn config_load_merges_toml_values() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");
        std::fs::write(
            &path,
            r#"
            [server]
            host = "0.0.0.0"
            port = 9000
            "#,
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 9000);
        // Non-overridden fields keep defaults.
        assert_eq!(config.server.jwt_secret, "dev-secret-change-me");
        assert!(config.server.database_url.contains("postgres://"));
    }

    #[test]
    fn config_does_not_override_jwt_secret_with_default_value() {
        // If the TOML literally sets jwt_secret to the default string,
        // Config::load must NOT treat that as an override (it skips it).
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");
        std::fs::write(
            &path,
            "[server]\njwt_secret = \"dev-secret-change-me\"\n",
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.server.jwt_secret, "dev-secret-change-me");
    }

    #[test]
    fn config_load_merges_ssh_section() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");
        std::fs::write(
            &path,
            r#"
            [server]
            host = "0.0.0.0"

            [server.ssh]
            enabled = true
            host = "0.0.0.0"
            port = 2224
            host_key = "/tmp/custom_host_key"
            "#,
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert!(config.server.ssh.enabled);
        assert_eq!(config.server.ssh.host, "0.0.0.0");
        assert_eq!(config.server.ssh.port, 2224);
        assert_eq!(
            config.server.ssh.host_key.as_deref(),
            Some(std::path::Path::new("/tmp/custom_host_key"))
        );
        // Defaults are preserved for fields not present in the file.
        assert_eq!(config.server.port, 3000);
    }

    #[test]
    fn config_ssh_defaults_are_disabled() {
        let missing = PathBuf::from("/nonexistent/gita.toml");
        let config = Config::load(&missing).unwrap();
        assert!(!config.server.ssh.enabled);
        assert_eq!(config.server.ssh.port, 2222);
        assert!(config.server.ssh.host_key.is_none());
    }

    #[test]
    fn config_pages_enabled_defaults_on_and_can_be_turned_off() {
        // Missing file -> default on.
        let missing = PathBuf::from("/nonexistent/gita.toml");
        let config = Config::load(&missing).unwrap();
        assert!(config.server.pages_enabled);
        assert_eq!(config.server.pages_url_prefix, "");

        // Explicitly false -> master switch off (the asymmetric SSH merge
        // pattern would have ignored this; presence-based merging must not).
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");
        std::fs::write(
            &path,
            "[server]\npages_enabled = false\n",
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert!(!config.server.pages_enabled, "pages_enabled=false must be honored");

        // Explicitly true -> on.
        std::fs::write(&path, "[server]\npages_enabled = true\n").unwrap();
        let config = Config::load(&path).unwrap();
        assert!(config.server.pages_enabled);
    }

    #[test]
    fn config_pages_url_prefix_is_loaded() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("gita.toml");
        std::fs::write(
            &path,
            "[server]\npages_url_prefix = \"/p\"\n",
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.server.pages_url_prefix, "/p");
    }

    /// Load the shipped example config and assert the Pages keys are where the
    /// code reads them.
    ///
    /// Regression test: TOML keys are scoped to the most recent table header,
    /// so placing `pages_enabled` after `[server.ssh]` silently parses it as
    /// `server.ssh.pages_enabled` — leaving `Config::load` to see nothing and
    /// the documented setting quietly inert. This test parses the actual
    /// example file the operator copies, so the two cannot drift apart.
    #[test]
    fn example_config_parses_pages_keys_under_server() {
        let example = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/config.example.toml");
        let text = std::fs::read_to_string(&example)
            .unwrap_or_else(|e| panic!("read {}: {e}", example.display()));

        let raw: toml::Value = toml::from_str(&text).expect("example config should be valid TOML");
        let server = raw.get("server").expect("example config should have [server]");
        assert!(
            server.get("pages_enabled").is_some(),
            "pages_enabled must live under [server], not [server.ssh] — TOML keys \
             belong to the preceding table header, so a misplaced key is silently ignored"
        );

        // And it must survive the real loader with the documented value.
        let config = Config::load(&example).unwrap();
        assert!(
            config.server.pages_enabled,
            "example config documents pages_enabled = true; loading it must keep Pages on"
        );
        assert_eq!(config.server.pages_url_prefix, "");
    }

    /// The Docker entrypoint generates config.toml from env vars; its Pages
    /// keys must land under `[server]` too.
    ///
    /// Same TOML table-scoping trap as the example config, in a shell script
    /// that no Rust test otherwise touches — the bug is invisible until
    /// someone tries to turn Pages off in a container and it stays on.
    #[test]
    fn docker_entrypoint_generates_pages_keys_under_server() {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docker/docker-entrypoint.sh");
        let text = std::fs::read_to_string(&script)
            .unwrap_or_else(|e| panic!("read {}: {e}", script.display()));

        // Locate the generated [server] block and the [server.ssh] header.
        let server_at = text.find("\n[server]\n").expect("entrypoint should emit [server]");
        let ssh_at = text
            .find("\n[server.ssh]\n")
            .expect("entrypoint should emit [server.ssh]");
        let pages_at = text
            .find("pages_enabled =")
            .expect("entrypoint should emit pages_enabled");
        assert!(
            pages_at > server_at && pages_at < ssh_at,
            "pages_enabled is emitted at offset {pages_at}, which falls outside the \
             [server] table ({server_at}..{ssh_at}) — TOML scopes keys to the most \
             recent table header, so it would be read as server.ssh.pages_enabled \
             and silently ignored"
        );
    }

    /// The webhook signature must be a real HMAC-SHA256 over the exact body,
    /// because that is what a receiver recomputes to verify the payload.
    #[test]
    fn webhook_signature_is_hmac_sha256_of_the_body() {
        let body = r#"{"event":"push","repo_id":7}"#;
        let sig = sign_webhook_payload("s3cret", body);
        // Known-answer: HMAC-SHA256("s3cret", body) computed independently.
        assert_eq!(sig.len(), 64, "sha256 hex digest is 64 chars");
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));

        // Stable across calls (deterministic, not random).
        assert_eq!(sig, sign_webhook_payload("s3cret", body));

        // Different secret -> different signature.
        assert_ne!(sig, sign_webhook_payload("other", body));
        // Different body -> different signature.
        assert_ne!(sig, sign_webhook_payload("s3cret", "{}"));
    }

    /// Cross-check against a hand-computed HMAC-SHA256 to catch an accidental
    /// change of algorithm or digest encoding.
    #[test]
    fn webhook_signature_matches_known_hmac_vector() {
        // RFC 4231 test case 1: key = 20 bytes of 0x0b, data = "Hi There".
        let key = "\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b";
        let expected = "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7";
        assert_eq!(sign_webhook_payload(key, "Hi There"), expected);
    }
}