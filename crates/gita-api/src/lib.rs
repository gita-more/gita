//! REST API for Gita.
//!
//! This crate exposes the `/api/*` HTTP endpoints, covering:
//!
//! - authentication (register, login, PAT tokens)
//! - repositories (CRUD, browse tree, read files, commits)
//! - pull requests (create, list, get, merge, close)
//! - AI reviews (trigger, list, get)
//! - issues (create, list, get, close)
//! - semantic code search
//! - event stream (AI-readable)
//!
//! The API follows a GitHub-like style.

pub mod auth;
pub mod docs;
pub mod error;
pub mod extract;
pub mod routes;
pub mod search;
pub mod state;

pub use auth::Auth;
pub use error::{ApiError, ApiResult, MessageResponse, message};
pub use extract::{AuthUserExtractor, OptionalAuthUser};
pub use search::{Fts5Backend, SearchBackend, SearchHit, SearchService};
pub use state::AppState;

/// Build the full API router.
pub fn api_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(routes::auth_routes())
        .merge(routes::repo_routes())
        .merge(routes::pr_routes())
        .merge(routes::review_routes())
        .merge(routes::issue_routes())
        .merge(routes::search_routes())
        .merge(routes::event_routes())
        .merge(routes::misc_routes())
        .merge(docs::docs_routes())
        .with_state(state)
}