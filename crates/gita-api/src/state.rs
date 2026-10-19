//! Shared API state.

use crate::auth::Auth;
use gita_db::Db;
use gita_events::EventBus;
use gita_git::GitManager;
use std::sync::Arc;

/// Shared state for all API handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub auth: Arc<Auth>,
    pub git: Arc<GitManager>,
    pub bus: EventBus,
    /// Per-IP login rate limiter. `None` means rate limiting is disabled.
    pub login_limiter: Option<crate::middleware::SharedLimiter>,
}

impl AppState {
    pub fn new(db: Db, auth: Auth, git: GitManager, bus: EventBus) -> Self {
        Self {
            db,
            auth: Arc::new(auth),
            git: Arc::new(git),
            bus,
            login_limiter: None,
        }
    }

    /// Set the login rate limiter (called by `gita-server::run`).
    pub fn with_login_limiter(mut self, limiter: crate::middleware::SharedLimiter) -> Self {
        self.login_limiter = Some(limiter);
        self
    }
}