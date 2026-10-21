//! Shared API state.

use crate::auth::Auth;
use gita_db::Db;
use gita_events::EventBus;
use gita_git::GitManager;
use std::path::PathBuf;
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
    /// Per-IP registration rate limiter, kept separate from the login one:
    /// registration performs an Argon2 hash on every call, and sharing a
    /// bucket with login would let one endpoint's traffic starve the other.
    /// `None` means rate limiting is disabled.
    pub register_limiter: Option<crate::middleware::SharedLimiter>,
    /// Directory where user avatar images are stored.
    pub avatar_dir: PathBuf,
    /// Return the password-reset token in the API response.
    ///
    /// Off by default. The response is uniform for existing and
    /// non-existent accounts, so handing back the token would let an
    /// unauthenticated caller enumerate accounts *and* take them over.
    /// Operators enable it for local development or for a deployment with
    /// no mail transport, where they read the token out of the response.
    pub allow_insecure_password_reset: bool,
}

impl AppState {
    pub fn new(db: Db, auth: Auth, git: GitManager, bus: EventBus) -> Self {
        Self {
            db,
            auth: Arc::new(auth),
            git: Arc::new(git),
            bus,
            login_limiter: None,
            register_limiter: None,
            avatar_dir: PathBuf::from("data/avatars"),
            allow_insecure_password_reset: false,
        }
    }

    /// Set the login rate limiter (called by `gita-server::run`).
    pub fn with_login_limiter(mut self, limiter: crate::middleware::SharedLimiter) -> Self {
        self.login_limiter = Some(limiter);
        self
    }

    /// Set the registration rate limiter (called by `gita-server::run`).
    pub fn with_register_limiter(mut self, limiter: crate::middleware::SharedLimiter) -> Self {
        self.register_limiter = Some(limiter);
        self
    }

    /// Set the avatar storage directory.
    pub fn with_avatar_dir(mut self, dir: PathBuf) -> Self {
        self.avatar_dir = dir;
        self
    }

    /// Allow the password-reset endpoint to return the token in its
    /// response. Off by default — see the field's documentation.
    pub fn with_insecure_password_reset(mut self, allow: bool) -> Self {
        self.allow_insecure_password_reset = allow;
        self
    }
}
