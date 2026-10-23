//! Repository visibility checks for the REST API.
//!
//! The rule lives in [`gita_core::access`] and is shared with the
//! git-over-HTTP and git-over-SSH paths. This module is the REST
//! adapter around it: it loads the repository, applies the rule, and
//! turns a refusal into the response the REST API uses.
//!
//! The REST API answers **404** for both "no such repository" and "not
//! yours". Telling an anonymous caller that a repository exists but is
//! hidden is itself a disclosure. The git transports answer 401
//! instead, because a git client needs the challenge to be prompted for
//! credentials — but the decision behind the refusal is the same one.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use gita_core::access::Access;
use gita_core::Repository;

/// The id of an authenticated caller.
fn user_id(user: Option<&crate::extract::AuthUserExtractor>) -> Option<i64> {
    user.map(|u| u.id)
}

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
    ensure_readable(&state, &repo, user).await?;
    Ok(repo)
}

/// Reject a read of a repository the caller may not see.
///
/// A private repository is visible only to its owner (or, for a
/// repository in a group namespace, to the group's members). Note this
/// returns 404 rather than 403 for a non-owner: telling an anonymous
/// caller "this exists but you cannot see it" is itself a disclosure.
pub async fn ensure_readable(
    state: &AppState,
    repo: &Repository,
    user: Option<&crate::extract::AuthUserExtractor>,
) -> ApiResult<()> {
    if allowed(state, repo, user_id(user), Access::Read).await? {
        Ok(())
    } else {
        Err(ApiError::NotFound("repository not found".into()))
    }
}

/// Load a repository and enforce that the caller may **write** to it.
///
/// Writing means creating or mutating content that belongs to the
/// repository: pull requests, issues, comments, stars, forks, CI runs.
///
/// For a personal repository this means being its owner — being able to
/// *see* a public repository does not make it yours to modify. For a
/// repository in a group namespace, any member of the group may write.
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
    ensure_writable(&state, &repo, user).await?;
    Ok(repo)
}

/// Reject a write to a repository the caller does not own.
pub async fn ensure_writable(
    state: &AppState,
    repo: &Repository,
    user: &crate::extract::AuthUserExtractor,
) -> ApiResult<()> {
    if allowed(state, repo, Some(user.id), Access::Write).await? {
        Ok(())
    } else {
        Err(ApiError::NotFound("repository not found".into()))
    }
}

/// Load a repository and enforce that the caller may change its history or
/// refs — creating or deleting a branch or a tag, or pushing to it.
pub async fn load_pushable_repo(
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
    ensure_pushable(&state, &repo, user).await?;
    Ok(repo)
}

/// Reject a change to a repository's refs by someone without push rights.
pub async fn ensure_pushable(
    state: &AppState,
    repo: &Repository,
    user: &crate::extract::AuthUserExtractor,
) -> ApiResult<()> {
    if allowed(state, repo, Some(user.id), Access::Push).await? {
        Ok(())
    } else {
        Err(ApiError::NotFound("repository not found".into()))
    }
}

/// Load a repository and enforce that the caller may administer it —
/// renaming it, transferring it, changing its visibility, deleting it,
/// or managing its webhooks.
pub async fn load_adminable_repo(
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
    ensure_adminable(&state, &repo, user).await?;
    Ok(repo)
}

/// Reject an administrative change by someone who is not the repository's
/// owner (for a group repository, the group's owner).
pub async fn ensure_adminable(
    state: &AppState,
    repo: &Repository,
    user: &crate::extract::AuthUserExtractor,
) -> ApiResult<()> {
    if allowed(state, repo, Some(user.id), Access::Admin).await? {
        Ok(())
    } else {
        Err(ApiError::NotFound("repository not found".into()))
    }
}

/// Apply the shared access rule.
async fn allowed(
    state: &AppState,
    repo: &Repository,
    user_id: Option<i64>,
    action: Access,
) -> ApiResult<bool> {
    Ok(state.db.can_access(repo, user_id, action).await?)
}
