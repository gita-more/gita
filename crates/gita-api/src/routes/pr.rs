//! Pull request handlers: create, list, get, merge, close.
//!
//! PRs are served through the single catch-all route in
//! [`crate::routes::repo`]; the two `*_dispatch` functions below are called
//! from that dispatcher once the path's `prs` marker segment has been
//! split off. `rest` is everything after the marker.

use crate::error::{ApiError, ApiResult};
use crate::extract::{AuthUserExtractor, OptionalAuthUser};
use crate::state::AppState;
use axum::response::IntoResponse;
use gita_core::{EventType, PrStatus, PullRequest};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct CreatePrRequest {
    pub title: String,
    pub body: Option<String>,
    pub source_branch: String,
    pub target_branch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PrResponse {
    pub id: i64,
    pub number: i64,
    pub title: String,
    pub body: String,
    pub source_branch: String,
    pub target_branch: String,
    pub status: String,
    pub author: String,
    pub base_sha: String,
    pub head_sha: String,
    pub created_at: String,
    pub updated_at: String,
    /// Files that conflict when merging `head_sha` into `target_branch`
    /// (empty when the PR can be merged cleanly or is not open).
    pub conflicts: Vec<String>,
    pub merged_at: Option<String>,
}

impl From<PullRequest> for PrResponse {
    fn from(p: PullRequest) -> Self {
        Self {
            id: p.id,
            number: p.number,
            title: p.title,
            body: p.body,
            source_branch: p.source_branch,
            target_branch: p.target_branch,
            status: p.status.to_string(),
            author: p.author_name,
            base_sha: p.base_sha,
            head_sha: p.head_sha,
            created_at: p.created_at.to_rfc3339(),
            updated_at: p.updated_at.to_rfc3339(),
            conflicts: Vec::new(),
            merged_at: p.merged_at.map(|d| d.to_rfc3339()),
        }
    }
}

impl PrResponse {
    /// Attach the conflict file list (computed lazily only for open PRs).
    async fn with_conflicts(
        self,
        state: &AppState,
        owner: &str,
        repo_name: &str,
    ) -> ApiResult<Self> {
        if self.status != "open" {
            return Ok(self);
        }
        match state
            .git
            .check_merge_conflict(owner, repo_name, &self.target_branch, &self.head_sha)
        {
            Ok(files) => Ok(Self {
                conflicts: files,
                ..self
            }),
            Err(_) => Ok(self),
        }
    }
}

/// GET dispatch for `/api/repos/{owner}/{repo}/prs[...]`.
///
/// `rest` is the path after the `prs` marker: `""` (list), `{number}`,
/// `{number}/diff`, `{number}/files`, `{number}/comments`.
pub(crate) async fn get_dispatch(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
    rest: &str,
    q: &std::collections::HashMap<String, String>,
) -> ApiResult<axum::response::Response> {
    let segs: Vec<&str> = rest.split('/').collect();
    match segs.as_slice() {
        [""] => {
            let res = list_prs(
                state,
                user,
                owner,
                repo,
                q.get("status").cloned(),
                q.get("limit").and_then(|v| v.parse::<usize>().ok()),
            )
            .await?;
            Ok(res.into_response())
        }
        [number] => {
            let number = parse_number(number)?;
            let res = get_pr(state, user, owner, repo, number).await?;
            Ok(res.into_response())
        }
        [number, "diff"] => {
            let number = parse_number(number)?;
            let res = pr_diff(state, user, owner, repo, number).await?;
            Ok(res.into_response())
        }
        [number, "files"] => {
            let number = parse_number(number)?;
            let res = pr_files(state, user, owner, repo, number).await?;
            Ok(res.into_response())
        }
        [number, "comments"] => {
            let number = parse_number(number)?;
            let res =
                crate::routes::comment::list_pr_comments(state, user, owner, repo, number).await?;
            Ok(res.into_response())
        }
        _ => Err(ApiError::NotFound("endpoint not found".into())),
    }
}

/// POST dispatch for `/api/repos/{owner}/{repo}/prs[...]`.
///
/// `rest` is the path after the `prs` marker: `""` (create), `{number}/merge`,
/// `{number}/close`, `{number}/comments`.
pub(crate) async fn post_dispatch(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    rest: &str,
    body: &[u8],
) -> ApiResult<axum::response::Response> {
    let segs: Vec<&str> = rest.split('/').collect();
    match segs.as_slice() {
        [""] => {
            let req: CreatePrRequest = serde_json::from_slice(body)
                .map_err(|e| ApiError::BadRequest(format!("invalid request body: {e}")))?;
            let res = create_pr(state, user, owner, repo, req).await?;
            Ok(res.into_response())
        }
        [number, "merge"] => {
            let number = parse_number(number)?;
            let res = merge_pr(state, user, owner, repo, number).await?;
            Ok(res.into_response())
        }
        [number, "close"] => {
            let number = parse_number(number)?;
            let res = close_pr(state, user, owner, repo, number).await?;
            Ok(res.into_response())
        }
        [number, "comments"] => {
            let number = parse_number(number)?;
            let req: crate::routes::comment::CreateCommentRequest = serde_json::from_slice(body)
                .map_err(|e| ApiError::BadRequest(format!("invalid request body: {e}")))?;
            let res =
                crate::routes::comment::create_pr_comment(state, user, owner, repo, number, req)
                    .await?;
            Ok(res.into_response())
        }
        _ => Err(ApiError::NotFound("endpoint not found".into())),
    }
}

fn parse_number(s: &str) -> ApiResult<i64> {
    s.parse()
        .map_err(|_| ApiError::BadRequest("invalid pull request number".into()))
}

async fn create_pr(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    req: CreatePrRequest,
) -> ApiResult<axum::Json<PrResponse>> {
    let repo = crate::routes::visibility::load_writable_repo(state, owner, repo, user).await?;

    let title = req.title.trim().to_string();
    if title.is_empty() {
        return Err(ApiError::BadRequest("title is required".into()));
    }
    let source_branch = req.source_branch.trim().to_string();
    let target_branch = req
        .target_branch
        .unwrap_or_else(|| repo.default_branch.clone());

    // Resolve SHAs.
    let head_sha = state
        .git
        .head_sha(owner, &repo.name, &source_branch)
        .map_err(|_| ApiError::BadRequest(format!("source branch not found: {source_branch}")))?;
    let base_sha = state
        .git
        .head_sha(owner, &repo.name, &target_branch)
        .map_err(|_| ApiError::BadRequest(format!("target branch not found: {target_branch}")))?;

    let pr = state
        .db
        .create_pull_request(
            repo.id,
            &title,
            req.body.as_deref().unwrap_or(""),
            &source_branch,
            &target_branch,
            user.id,
            &user.username,
            &base_sha,
            &head_sha,
        )
        .await?;

    state.bus.publish(
        EventType::PullRequestOpened,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({
            "pr": pr.number,
            "title": pr.title,
            "source": source_branch,
            "target": target_branch,
        }),
    );

    Ok(axum::Json(PrResponse::from(pr)))
}

async fn list_prs(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
    status: Option<String>,
    limit: Option<usize>,
) -> ApiResult<axum::Json<serde_json::Value>> {
    let repository =
        crate::routes::visibility::load_readable_repo(state, owner, repo, user.0.as_ref()).await?;
    let status = match status.as_deref() {
        None | Some("open") => Some(PrStatus::Open),
        Some("merged") => Some(PrStatus::Merged),
        Some("closed") => Some(PrStatus::Closed),
        Some("all") => None,
        Some(_) => return Err(ApiError::BadRequest("invalid status".into())),
    };
    let limit = limit.unwrap_or(30).min(200);
    let prs = state
        .db
        .list_pull_requests(repository.id, status, limit)
        .await?;
    let mut out = Vec::with_capacity(prs.len());
    for pr in prs {
        out.push(
            PrResponse::from(pr)
                .with_conflicts(state, owner, repo)
                .await?,
        );
    }
    Ok(axum::Json(serde_json::json!({
        "prs": out,
    })))
}

async fn get_pr(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
    number: i64,
) -> ApiResult<axum::Json<serde_json::Value>> {
    let repository =
        crate::routes::visibility::load_readable_repo(state, owner, repo, user.0.as_ref()).await?;
    let pr = state
        .db
        .get_pull_request(repository.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("pull request not found".into()))?;
    let resp = PrResponse::from(pr)
        .with_conflicts(state, owner, repo)
        .await?;
    Ok(axum::Json(serde_json::json!({
        "pr": resp,
    })))
}

async fn merge_pr(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    number: i64,
) -> ApiResult<axum::Json<serde_json::Value>> {
    let repository =
        crate::routes::visibility::load_writable_repo(state, owner, repo, user).await?;
    let pr = state
        .db
        .get_pull_request(repository.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("pull request not found".into()))?;
    if pr.status != PrStatus::Open {
        return Err(ApiError::Conflict("pull request is not open".into()));
    }

    // Claim the merge before touching git.
    //
    // Checking `status != Open` and merging afterwards is a race: two
    // requests both pass the check, both run git, and the second one
    // produces a *second* merge commit whose parents include the first
    // one's — the branch then carries a duplicate merge for one PR. The
    // state change is the only thing that can be made atomic, so it goes
    // first and decides the winner; a request that loses it never runs git.
    //
    // If the merge below fails, the claim is reverted so the PR can be
    // retried instead of being stuck "merged" with no merge commit.
    let claimed_at = chrono::Utc::now();
    if !state
        .db
        .transition_pr_status(pr.id, PrStatus::Open, PrStatus::Merged, Some(claimed_at))
        .await?
    {
        return Err(ApiError::Conflict(
            "pull request is already being merged or closed".into(),
        ));
    }

    // Detect conflicts before attempting the merge so a conflicting PR
    // returns a clear 409 instead of a generic 500.
    let conflicts = state
        .git
        .check_merge_conflict(owner, repo, &pr.target_branch, &pr.head_sha)
        .map_err(ApiError::from)?;
    if !conflicts.is_empty() {
        revert_merge_claim(state, pr.id).await;
        return Err(ApiError::Conflict(format!(
            "merge conflict in: {}",
            conflicts.join(", ")
        )));
    }

    // Resolve merger identity (name + email) for the merge commit author.
    // Without an explicit identity, `git commit-tree` falls back to whatever
    // identity is in the surrounding environment's git config, which on a
    // fresh runner container may be unset and cause the merge to fail.
    let author_email = match state.db.get_user_by_id(user.id).await? {
        Some(u) => u.email,
        None => format!("{}@users.noreply.gita.local", user.username),
    };
    let author_name = user.username.clone();

    // Perform the merge (fast-forward or merge commit).
    if let Err(e) = state.git.merge_branch(
        owner,
        repo,
        &pr.target_branch,
        &pr.head_sha,
        &author_name,
        &author_email,
    ) {
        revert_merge_claim(state, pr.id).await;
        return Err(ApiError::from(e));
    }

    let new_head = state
        .git
        .head_sha(owner, repo, &pr.target_branch)
        .map_err(ApiError::from)?;

    state.bus.publish(
        EventType::PullRequestMerged,
        Some(repository.id),
        Some(user.id),
        serde_json::json!({
            "pr": pr.number,
            "title": pr.title,
            "merge_sha": new_head,
        }),
    );

    Ok(axum::Json(serde_json::json!({
        "message": "pull request merged",
        "merge_sha": new_head,
    })))
}

/// Undo the "claimed for merge" status change when the git step fails.
///
/// Only a *failure path* calls this: the claim already moved the PR to
/// merged, so leaving it there would strand a PR whose branch never got
/// the merge. The revert is best-effort by design — if it cannot run
/// (database gone), the error that is about to be returned is the one
/// worth reporting, and the next merge attempt will see a non-open PR
/// rather than silently merging twice.
async fn revert_merge_claim(state: &AppState, pr_id: i64) {
    if let Err(e) = state
        .db
        .transition_pr_status(pr_id, PrStatus::Merged, PrStatus::Open, None)
        .await
    {
        tracing::error!("failed to revert the merge claim on PR {pr_id}: {e}");
    }
}

async fn pr_diff(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
    number: i64,
) -> ApiResult<axum::Json<serde_json::Value>> {
    let repository =
        crate::routes::visibility::load_readable_repo(state, owner, repo, user.0.as_ref()).await?;
    let pr = state
        .db
        .get_pull_request(repository.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("pull request not found".into()))?;
    let diff = state
        .git
        .diff_between(owner, repo, &pr.base_sha, &pr.head_sha)
        .map_err(ApiError::from)?;
    Ok(axum::Json(serde_json::json!({ "diff": diff })))
}

async fn pr_files(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
    number: i64,
) -> ApiResult<axum::Json<serde_json::Value>> {
    let repository =
        crate::routes::visibility::load_readable_repo(state, owner, repo, user.0.as_ref()).await?;
    let pr = state
        .db
        .get_pull_request(repository.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("pull request not found".into()))?;
    let files = state
        .git
        .diff_files_between(owner, repo, &pr.base_sha, &pr.head_sha)
        .map_err(ApiError::from)?;
    Ok(axum::Json(serde_json::json!({ "files": files })))
}

async fn close_pr(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    number: i64,
) -> ApiResult<axum::Json<crate::error::MessageResponse>> {
    let repo = crate::routes::visibility::load_writable_repo(state, owner, repo, user).await?;
    let pr = state
        .db
        .get_pull_request(repo.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("pull request not found".into()))?;
    if pr.status != PrStatus::Open {
        return Err(ApiError::Conflict("pull request is not open".into()));
    }
    // Same claim as the merge path: closing is a status transition, and
    // two concurrent closes (or a close racing a merge) must not both
    // report success. Only the request that moves open → closed publishes
    // the event.
    if !state
        .db
        .transition_pr_status(pr.id, PrStatus::Open, PrStatus::Closed, None)
        .await?
    {
        return Err(ApiError::Conflict("pull request is not open".into()));
    }

    state.bus.publish(
        EventType::PullRequestClosed,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({"pr": pr.number, "title": pr.title}),
    );

    Ok(axum::Json(crate::error::MessageResponse {
        message: "pull request closed".to_string(),
    }))
}
