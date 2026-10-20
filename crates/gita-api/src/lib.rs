//! REST API for Gita.
//!
//! This crate exposes the `/api/*` HTTP endpoints, covering:
//!
//! - authentication (register, login, PAT tokens)
//! - repositories (CRUD, browse tree, read files, commits)
//! - pull requests (create, list, get, merge, close)
//! - issues (create, list, get, close)
//! - event stream (AI-readable)
//!
//! The API follows a GitHub-like style.

pub mod auth;
pub mod docs;
pub mod error;
pub mod extract;
pub mod middleware;
pub mod routes;
pub mod state;

pub use auth::Auth;
pub use error::{ApiError, ApiResult, MessageResponse, message};
pub use extract::{AuthUserExtractor, OptionalAuthUser};
pub use middleware::{SharedLimiter, build_limiter};
pub use routes::pages_site::{PagesOptions, pages_site_routes};
pub use state::AppState;

/// Build the full API router.
pub fn api_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(routes::auth_routes())
        .merge(routes::repo_routes())
        .merge(routes::pr_routes())
        .merge(routes::issue_routes())
        .merge(routes::comment_routes())
        .merge(routes::group_routes())
        .merge(routes::event_routes())
        .merge(routes::misc_routes())
        .merge(routes::pipeline_routes())
        .merge(routes::pages_routes())
        .merge(routes::search_routes())
        .merge(routes::avatar_routes())
        .merge(routes::ssh_key_routes())
        .merge(docs::docs_routes())
        .with_state(state)
}

/// Build the Pages static-site serving router, mounted at the server root
/// (not under `/api`).
pub fn pages_router(state: AppState, opts: PagesOptions) -> axum::Router {
    pages_site_routes(opts).with_state(state)
}