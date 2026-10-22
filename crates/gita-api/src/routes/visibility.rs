//! Repository visibility checks for the REST API.
//!
//! The git-over-HTTP and SSH paths already enforce "public repositories are
//! anonymously readable, private ones are owner-only". The REST API did not:
//! its repo-scoped read handlers took no auth extractor and never looked at
//! `is_private`, so an anonymous caller could read the files, commits, pull
//! requests and issues of a private repository — while the README advertised
//! the opposite.
//!
//! This module centralises the rule so every read path applies the same
//! check instead of each handler re-deriving it (and forgetting).

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use gita_core::Repository;

/// Load a repository and enforce that the caller may read it.
///
/// `user` is the authenticated caller, or `None` for an anonymous request. A
/// private repository is readable only by its owner. For a public
/// repository, authentication is not required — matching the git protocol's
/// behaviour, where a public repo is anonymously clonable.
pub async fn load_readable_repo(
    state: &AppState,
    owner: &str,
    name: &str,
    user: Option<&crate::extract::AuthUserExtractor>,
) -> ApiResult<Repository> {
    let repo = state
        .db
        .get_repository_by_name(owner, name)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    ensure_readable(&repo, user)?;
    Ok(repo)
}

/// Reject a read of a repository the caller may not see.
///
/// A private repository is visible only to its owner. Note this returns 404
/// rather than 403 for a non-owner: telling an anonymous caller "this
/// exists but you cannot see it" is itself a disclosure, and 404 is what the
/// git-over-HTTP path effectively communicates.
pub fn ensure_readable(
    repo: &Repository,
    user: Option<&crate::extract::AuthUserExtractor>,
) -> ApiResult<()> {
    if !repo.is_private {
        return Ok(());
    }
    match user {
        Some(u) if u.id == repo.owner_id => Ok(()),
        _ => Err(ApiError::NotFound("repository not found".into())),
    }
}

/// Whether the caller may read this repository. Used by listing endpoints to
/// filter rather than to reject.
pub fn can_read(repo: &Repository, user: Option<&crate::extract::AuthUserExtractor>) -> bool {
    if !repo.is_private {
        return true;
    }
    matches!(user, Some(u) if u.id == repo.owner_id)
}

/// Load a repository and enforce that the caller may **write** to it.
///
/// Writing means creating or mutating content that belongs to the
/// repository: pull requests, issues, comments, stars, forks, CI runs.
///
/// The rule is the same one the git protocol enforces (`gita-http` and
/// `gita-ssh` both allow a write only for `owner_id`): a private
/// repository is writable only by its owner, and a public repository is
/// writable only by its owner as well — being able to *see* a public
/// repository does not make it yours to modify.
///
/// A non-owner gets 404 rather than 403 for the same reason as the read
/// path: "this exists but you cannot see it" is itself a disclosure.
pub async fn load_writable_repo(
    state: &AppState,
    owner: &str,
    name: &str,
    user: &crate::extract::AuthUserExtractor,
) -> ApiResult<Repository> {
    let repo = state
        .db
        .get_repository_by_name(owner, name)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    ensure_writable(&repo, user)?;
    Ok(repo)
}

/// Reject a write to a repository the caller does not own.
pub fn ensure_writable(
    repo: &Repository,
    user: &crate::extract::AuthUserExtractor,
) -> ApiResult<()> {
    if user.id == repo.owner_id {
        return Ok(());
    }
    Err(ApiError::NotFound("repository not found".into()))
}
