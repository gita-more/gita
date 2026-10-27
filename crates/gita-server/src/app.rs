//! The assembled application state and router construction.

use anyhow::{Context, Result};

use gita_api::auth::Auth;

use gita_db::Db;

use gita_events::EventBus;

use gita_git::GitManager;

use gita_http::GitHttpState;

use std::sync::Arc;

use tracing::{info, warn};

use crate::config::{PASSWORD_RESET_RATE_BURST, PASSWORD_RESET_RATE_PER_SECOND, REGISTER_RATE_BURST, REGISTER_RATE_PER_SECOND};

use crate::runtime::find_web_dist;
use crate::shutdown;
use crate::sign_webhook_payload;
use crate::Config;

/// The assembled application state.
#[derive(Clone)]
pub struct App {
    pub db: Db,
    pub git: Arc<GitManager>,
    pub bus: EventBus,
    /// Background tasks that must finish before the process exits.
    ///
    /// The event row for something that just happened, and the webhooks
    /// that event fans out to, are written from background tasks. A
    /// shutdown that did not wait for them would lose events that
    /// happened — and webhooks that were never delivered.
    pub background_tasks: shutdown::BackgroundTasks,
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
                .with_context(|| format!("connect to database {}", config.server.database_url))?,
        };

        let git = Arc::new(GitManager::new(repos_dir));
        // Persist every published event to the database so the `/api/events`
        // endpoint and AI-readable event stream reflect real activity.
        // Also fan out repository-scoped events to any registered webhooks.
        //
        // Both happen in background tasks, tracked so that a shutdown can
        // wait for them: an event that happened and a webhook that was
        // never delivered are both losses the user cannot see the
        // boundary of.
        let event_db = db.clone();
        let event_git_root = git.root().to_path_buf();
        let background_tasks = shutdown::BackgroundTasks::new();
        let hook_tasks = background_tasks.clone();
        let bus = EventBus::new(Some(Arc::new(move |event: gita_core::Event| {
            let db = event_db.clone();
            let git_root = event_git_root.clone();
            let tasks = hook_tasks.clone();
            let outer_tasks = tasks.clone();
            tasks.spawn(async move {
                // An event that already carries a database id was inserted by
                // its publisher — the push handler needs the id to set
                // `pipeline_runs.trigger_event_id`, and it cannot wait for
                // this hook (which is synchronous and cannot return the id
                // it generates). Skip the insert so the row is not written
                // twice. The webhook fan-out below still happens.
                if event.id == 0 {
                    if let Err(e) = db.insert_event(&event).await {
                        warn!("failed to persist event: {e}");
                    }
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
                                    && !hook
                                        .events
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
                                // The delivery is tracked too: a webhook
                                // that was never sent is a webhook the
                                // user was told about and never got.
                                let delivery_tasks = outer_tasks.clone();
                                delivery_tasks.spawn(async move {
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

        let api_state =
            gita_api::AppState::new(db.clone(), auth.clone(), (*git).clone(), bus.clone())
                .with_avatar_dir(config.server.data_dir.join("avatars"))
                .with_insecure_password_reset(config.server.allow_insecure_password_reset)
                .with_ssh_config(
                    config.server.ssh.enabled,
                    config.server.ssh.host.clone(),
                    config.server.ssh.port,
                    config.server.ssh.public_host.clone(),
                );

        let git_http_state = GitHttpState {
            git: git.clone(),
            db: db.clone(),
            bus: bus.clone(),
            auth: Arc::new(auth.clone()),
        };

        // Ensure the SSH host key exists (generated on first run) so a
        // later `run()` can start the SSH listener with a stable identity.
        if config.server.ssh.enabled {
            let host_key_path =
                config.server.ssh.host_key.clone().unwrap_or_else(|| {
                    config.server.data_dir.join(gita_ssh::DEFAULT_HOST_KEY_NAME)
                });
            gita_ssh::load_or_create_host_key(&host_key_path)?;
        }

        Ok(Self {
            db,
            git,
            bus,
            background_tasks,
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
    /// The registration and password-reset limiters are the default ones;
    /// use [`App::router_with_limiters`] to control all three.
    pub fn router_with_limiter(&self, limiter: gita_api::SharedLimiter) -> axum::Router {
        self.router_with_limiters(
            limiter,
            gita_api::build_limiter(REGISTER_RATE_PER_SECOND, REGISTER_RATE_BURST),
            gita_api::build_limiter(PASSWORD_RESET_RATE_PER_SECOND, PASSWORD_RESET_RATE_BURST),
        )
    }

    /// Build the full axum router with explicit per-IP limiters for login,
    /// registration and password reset.
    ///
    /// The three get separate buckets on purpose. Registration is the more
    /// expensive endpoint (every call runs an Argon2 hash) and it creates a
    /// new account per call, so a script could otherwise fill the users
    /// table at full request rate — while brute-forcing a *known* account
    /// was limited. Sharing one bucket would also let login traffic starve
    /// registration (and the reverse). The password-reset endpoints are
    /// unauthenticated and write a row and a log line per request for an
    /// existing account, so they get their own bucket for the same reason.
    pub fn router_with_limiters(
        &self,
        login_limiter: gita_api::SharedLimiter,
        register_limiter: gita_api::SharedLimiter,
        password_reset_limiter: gita_api::SharedLimiter,
    ) -> axum::Router {
        // Install the limiters into AppState so the handlers can check the
        // buckets; the rest of the API is unaffected.
        let api_state = self
            .api_state
            .clone()
            .with_login_limiter(login_limiter)
            .with_register_limiter(register_limiter)
            .with_password_reset_limiter(password_reset_limiter);
        let api_router = gita_api::api_router(api_state.clone());
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

        // Merge order: api -> pages -> SPA -> git middleware (last, wraps everything)
        axum::Router::new()
            .merge(api_router)
            .merge(pages_router)
            .merge(web_router)
            .layer(axum::middleware::from_fn({
                let state = self.git_http_state.clone();
                move |req, next| gita_http::git_middleware(state.clone(), req, next)
            }))
    }
}
