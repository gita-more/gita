//! GitHub-Pages-style configuration endpoints.
//!
//! These endpoints manage the per-repository "Deploy from a branch"
//! publishing configuration. The *serving* side of Pages lives in
//! [`pages_site_routes`] and is mounted at the server root, not under
//! `/api`, because a static site must be reachable at `/{owner}/{repo}/`.

use crate::error::{ApiError, ApiResult};
use crate::extract::AuthUserExtractor;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

/// Per-repository Pages configuration as exposed to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct PagesResponse {
    pub enabled: bool,
    pub branch: String,
    pub path: String,
    /// The public URL under which the site is served, e.g. `/alice/docs/`.
    /// Server-computed so the frontend never re-derives the path scheme.
    pub url: String,
    /// Whether `branch` actually resolves in git. Lets the UI grey out Save
    /// against a typo'd branch instead of discovering it on submit.
    pub available: bool,
}

/// Request body for PUT `/api/repos/{owner}/{repo}/pages`.
#[derive(Debug, Deserialize)]
pub struct PutPagesRequest {
    pub enabled: bool,
    /// Defaults to the repository's default branch when absent.
    pub branch: Option<String>,
    /// Defaults to `/`. Only `/` and `/docs` are allowed, matching GitHub.
    pub path: Option<String>,
}

pub fn pages_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/repos/{owner}/{repo}/pages",
            get(get_pages).put(put_pages).delete(delete_pages),
        )
        .route("/api/repos/{owner}/{repo}/pages/status", get(get_pages_status))
}

async fn get_pages(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<PagesResponse>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    // A private repo's configured branch is itself information worth
    // protecting; only the owner may read it. Public repos are readable by
    // any authenticated user.
    if repository.is_private && repository.owner_id != user.id {
        return Err(ApiError::Forbidden(
            "only the repository owner can view pages for a private repository".into(),
        ));
    }
    Ok(Json(response_for(&state, &repository).await))
}

/// Owner-only write: Pages configuration changes the public surface of a repo.
async fn put_pages(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
    Json(req): Json<PutPagesRequest>,
) -> ApiResult<Json<PagesResponse>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if repository.owner_id != user.id {
        return Err(ApiError::Forbidden(
            "only the repository owner can configure pages".into(),
        ));
    }

    let branch = req.branch.unwrap_or_else(|| repository.default_branch.clone());
    let path = req.path.unwrap_or_else(|| "/".to_string());

    validate_pages_path(&path)?;
    validate_branch_name(&branch)?;
    if req.enabled && !state.git.ref_exists(&owner, &repo, &branch) {
        return Err(ApiError::BadRequest(
            "branch not found: no such branch in this repository".into(),
        ));
    }

    state
        .db
        .upsert_pages(repository.id, req.enabled, &branch, &path)
        .await?;

    Ok(Json(response_for(&state, &repository).await))
}

/// Disable Pages. Deleting the row is equivalent to disabled-and-defaulted,
/// so no separate tombstone is needed.
async fn delete_pages(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if repository.owner_id != user.id {
        return Err(ApiError::Forbidden(
            "only the repository owner can configure pages".into(),
        ));
    }
    state.db.delete_pages(repository.id).await?;
    Ok(crate::error::message("pages disabled"))
}

/// Status endpoint without any Pages configuration body, so the repo page can
/// know whether to show a "Pages" entry even before the config exists.
async fn get_pages_status(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if repository.is_private && repository.owner_id != user.id {
        return Err(ApiError::Forbidden(
            "only the repository owner can view pages for a private repository".into(),
        ));
    }
    let row = state.db.get_pages(repository.id).await?;
    Ok(Json(serde_json::json!({
        "configured": row.is_some(),
        "enabled": row.as_ref().map(|r| r.enabled).unwrap_or(false),
    })))
}

/// Build the response for a repository, synthesizing defaults when no row
/// exists (so the UI can render the form from defaults without a separate
/// "create" call).
async fn response_for(state: &AppState, repository: &gita_core::Repository) -> PagesResponse {
    let row = state.db.get_pages(repository.id).await.ok().flatten();
    let (enabled, branch, path) = match row {
        Some(r) => (r.enabled, r.branch, r.path),
        None => (false, repository.default_branch.clone(), "/".to_string()),
    };
    let url = if path == "/" {
        format!("/{}/{}/", repository.owner_name, repository.name)
    } else {
        format!("/{}/{}/{}", repository.owner_name, repository.name, path.trim_matches('/'))
    };
    PagesResponse {
        enabled,
        branch: branch.clone(),
        path,
        url,
        available: state.git.ref_exists(&repository.owner_name, &repository.name, &branch),
    }
}

/// Validate the publishing root. GitHub allows only `/` and `/docs`; we
/// mirror that exactly rather than accepting arbitrary directories.
pub(crate) fn validate_pages_path(path: &str) -> ApiResult<()> {
    let trimmed = path.trim();
    if trimmed == "/" || trimmed == "/docs" {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "invalid pages path: expected \"/\" or \"/docs\"".into(),
        ))
    }
}

/// Validate a git branch name against ref rules, without shelling out to
/// `git check-ref-format` on user-supplied input. Mirrors the pure-function
/// style of `gita_db::ssh_key_is_valid`.
pub(crate) fn validate_branch_name(branch: &str) -> ApiResult<()> {
    if branch.is_empty() {
        return Err(ApiError::BadRequest("invalid branch name: empty".into()));
    }
    if branch.starts_with('-') || branch.ends_with('/') || branch.ends_with(".lock") {
        return Err(ApiError::BadRequest("invalid branch name".into()));
    }
    if branch.contains("..")
        || branch.contains(' ')
        || branch.contains('~')
        || branch.contains('^')
        || branch.contains(':')
        || branch.contains('?')
        || branch.contains('*')
        || branch.contains('[')
        || branch.contains('\\')
        || branch.contains("@{")
    {
        return Err(ApiError::BadRequest("invalid branch name".into()));
    }
    // Branches, not full refs or object ids.
    if branch == "@" || branch.contains("refs/") {
        return Err(ApiError::BadRequest("invalid branch name".into()));
    }
    // A branch name that happens to be a full SHA is ambiguous; reject it.
    if branch.len() == 40 && branch.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::BadRequest(
            "invalid branch name: use a branch name, not a commit SHA".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_path_accepts_only_root_and_docs() {
        assert!(validate_pages_path("/").is_ok());
        assert!(validate_pages_path("/docs").is_ok());
        // A trailing slash, case variant, or missing slash must be rejected.
        assert!(validate_pages_path("/docs/").is_err());
        assert!(validate_pages_path("/Docs").is_err());
        assert!(validate_pages_path("docs").is_err());
        assert!(validate_pages_path("//").is_err());
        assert!(validate_pages_path("/dist").is_err());
        assert!(validate_pages_path("").is_err());
    }

    #[test]
    fn branch_name_validation_table() {
        // Valid names.
        for ok in ["main", "feature/x", "dev-1", "user/name"] {
            assert!(validate_branch_name(ok).is_ok(), "{ok:?} should be valid");
        }
        // Invalid names, each rejected.
        for bad in [
            "-lead",        // starts with hyphen
            "a..b",         // consecutive dots
            "a b",          // space
            "a~b",          // tilde
            "a^b",          // caret
            "a:b",          // colon
            "a?b",          // question mark
            "a*b",          // asterisk
            "a[b",          // bracket
            "a\\b",         // backslash
            "a@{b",         // at-brace
            "trailing/",    // ends with slash
            "x.lock",       // ends with .lock
            "@",            // the single at
            "refs/heads/x", // full ref, not a branch name
            &"a".repeat(40), // a branch that is also a full SHA
        ] {
            assert!(validate_branch_name(bad).is_err(), "{bad:?} should be invalid");
        }
    }
}