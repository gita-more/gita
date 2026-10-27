//! Route modules.

pub mod admin;
pub mod auth;
pub mod avatar;
pub mod comment;
pub mod group;
pub mod issue;
pub mod misc;
pub mod pages;
pub mod pages_site;
pub mod pipeline;
pub mod pr;
pub mod repo;
pub mod resolve;
pub mod search;
pub mod ssh_key;
pub mod visibility;

// Re-export route functions.
pub use admin::admin_routes;
pub use auth::auth_routes;
pub use avatar::avatar_routes;
pub use comment::comment_routes;
pub use group::group_routes;
pub use issue::issue_routes;
pub use misc::event_routes;
pub use misc::misc_routes;
pub use repo::repo_routes;
pub use resolve::resolve_routes;
pub use search::search_routes;
pub use ssh_key::ssh_key_routes;
/// Map a git-layer error to an API error.
///
/// A missing commit, branch or ref is a 404 — the client asked for
/// something that does not exist. Only genuinely unexpected failures
/// become a 500. Reporting "invalid sha" as an internal error would tell
/// the client the *server* is broken when in fact it just asked for a
/// commit that was never written.
pub fn not_found_or_internal(what: &str, err: anyhow::Error) -> crate::error::ApiError {
    let msg = err.to_string();
    let lower = msg.to_ascii_lowercase();
    if lower.contains("not found")
        || lower.contains("invalid sha")
        || lower.contains("does not exist")
        || lower.contains("unknown revision")
        || lower.contains("no such ref")
    {
        crate::error::ApiError::NotFound(format!("{what} not found"))
    } else {
        crate::error::ApiError::Internal(msg)
    }
}
