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

use axum::response::IntoResponse;

pub use auth::Auth;
pub use error::{message, ApiError, ApiResult, MessageResponse};
pub use extract::{AuthUserExtractor, OptionalAuthUser, OptionalConnectInfo};
pub use middleware::{build_limiter, build_slow_refill_limiter, client_ip, SharedLimiter};
pub use routes::pages_site::{pages_site_routes, PagesOptions};
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
        // Catch-all for unknown API paths. Must be an explicit route (not a
        // `fallback`) because the server merges this router with `web_router`,
        // which itself carries a SPA fallback — two fallbacks cannot merge.
        // Concrete routes always win over this wildcard, so real endpoints
        // are unaffected. Returns JSON 404 instead of the SPA's HTML.
        .route("/api/{*rest}", axum::routing::any(fallback_404))
        .with_state(state)
}

/// Fallback handler for unknown API routes — returns JSON 404
/// instead of the SPA's HTML.
pub async fn fallback_404() -> impl axum::response::IntoResponse {
    ApiError::NotFound("API endpoint not found".into()).into_response()
}

/// Build the Pages static-site serving router, mounted at the server root
/// (not under `/api`).
pub fn pages_router(state: AppState, opts: PagesOptions) -> axum::Router {
    pages_site_routes(opts).with_state(state)
}
