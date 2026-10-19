//! Pull request routes: create, list, get, merge, close.

use crate::error::{ApiError, ApiResult};
use crate::extract::AuthUserExtractor;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use gita_core::{EventType, PrStatus, PullRequest};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct CreatePrRequest {
    pub title: String,
    pub body: Option<String>,
    pub source_branch: String,
    pub target_branch: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListPrQuery {
    pub status: Option<String>,
    pub limit: Option<usize>,
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
    async fn with_conflicts(self, state: &AppState, owner: &str, repo_name: &str) -> ApiResult<Self> {
        if self.status != "open" {
            return Ok(self);
        }
        match state
            .git
            .check_merge_conflict(owner, repo_name, &self.target_branch, &self.head_sha)
        {
            Ok(files) => Ok(Self { conflicts: files, ..self }),
            Err(_) => Ok(self),
        }
    }
}

pub fn pr_routes() -> Router<AppState> {
    Router::new()
        .route("/api/repos/{owner}/{repo}/prs", post(create_pr).get(list_prs))
        .route("/api/repos/{owner}/{repo}/prs/{number}", get(get_pr))
        .route("/api/repos/{owner}/{repo}/prs/{number}/merge", post(merge_pr))
        .route("/api/repos/{owner}/{repo}/prs/{number}/close", post(close_pr))
        .route("/api/repos/{owner}/{repo}/prs/{number}/diff", get(pr_diff))
        .route("/api/repos/{owner}/{repo}/prs/{number}/files", get(pr_files))
}

async fn create_pr(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
    Json(req): Json<CreatePrRequest>,
) -> ApiResult<Json<PrResponse>> {
    let repo = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;

    let title = req.title.trim().to_string();
    if title.is_empty() {
        return Err(ApiError::BadRequest("title is required".into()));
    }
    let source_branch = req.source_branch.trim().to_string();
    let target_branch = req.target_branch.unwrap_or_else(|| repo.default_branch.clone());

    // Resolve SHAs.
    let head_sha = state
        .git
        .head_sha(&owner, &repo.name, &source_branch)
        .map_err(|_| ApiError::BadRequest(format!("source branch not found: {source_branch}")))?;
    let base_sha = state
        .git
        .head_sha(&owner, &repo.name, &target_branch)
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

    Ok(Json(PrResponse::from(pr)))
}

async fn list_prs(
    State(state): State<AppState>,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<ListPrQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let status = match q.status.as_deref() {
        None | Some("open") => Some(PrStatus::Open),
        Some("merged") => Some(PrStatus::Merged),
        Some("closed") => Some(PrStatus::Closed),
        Some("all") => None,
        Some(_) => return Err(ApiError::BadRequest("invalid status".into())),
    };
    let limit = q.limit.unwrap_or(30).min(200);
    let prs = state.db.list_pull_requests(repository.id, status, limit).await?;
    let mut out = Vec::with_capacity(prs.len());
    for pr in prs {
        out.push(PrResponse::from(pr).with_conflicts(&state, &owner, &repo).await?);
    }
    Ok(Json(serde_json::json!({
        "prs": out,
    })))
}

async fn get_pr(
    State(state): State<AppState>,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let pr = state
        .db
        .get_pull_request(repository.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("pull request not found".into()))?;
    let resp = PrResponse::from(pr)
        .with_conflicts(&state, &owner, &repo)
        .await?;
    Ok(Json(serde_json::json!({
        "pr": resp,
    })))
}

async fn merge_pr(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let pr = state
        .db
        .get_pull_request(repository.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("pull request not found".into()))?;
    if pr.status != PrStatus::Open {
        return Err(ApiError::Conflict("pull request is not open".into()));
    }

    // Detect conflicts before attempting the merge so a conflicting PR
    // returns a clear 409 instead of a generic 500.
    let conflicts = state
        .git
        .check_merge_conflict(&owner, &repo, &pr.target_branch, &pr.head_sha)
        .map_err(ApiError::from)?;
    if !conflicts.is_empty() {
        return Err(ApiError::Conflict(format!(
            "merge conflict in: {}",
            conflicts.join(", ")
        )));
    }

    // Perform the merge (fast-forward or merge commit).
    state
        .git
        .merge_branch(&owner, &repo, &pr.target_branch, &pr.head_sha)
        .map_err(ApiError::from)?;

    let merged_at = chrono::Utc::now();
    state
        .db
        .update_pr_status(pr.id, PrStatus::Merged, Some(merged_at))
        .await?;

    let new_head = state
        .git
        .head_sha(&owner, &repo, &pr.target_branch)
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

    Ok(Json(serde_json::json!({
        "message": "pull request merged",
        "merge_sha": new_head,
    })))
}

async fn pr_diff(
    State(state): State<AppState>,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let pr = state
        .db
        .get_pull_request(repository.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("pull request not found".into()))?;
    let diff = state
        .git
        .diff_between(&owner, &repo, &pr.base_sha, &pr.head_sha)
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "diff": diff })))
}

async fn pr_files(
    State(state): State<AppState>,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let pr = state
        .db
        .get_pull_request(repository.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("pull request not found".into()))?;
    let files = state
        .git
        .diff_files_between(&owner, &repo, &pr.base_sha, &pr.head_sha)
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "files": files })))
}

async fn close_pr(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let repo = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let pr = state
        .db
        .get_pull_request(repo.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("pull request not found".into()))?;
    if pr.status != PrStatus::Open {
        return Err(ApiError::Conflict("pull request is not open".into()));
    }
    state
        .db
        .update_pr_status(pr.id, PrStatus::Closed, None)
        .await?;

    state.bus.publish(
        EventType::PullRequestClosed,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({"pr": pr.number, "title": pr.title}),
    );

    Ok(Json(crate::error::MessageResponse {
        message: "pull request closed".to_string(),
    }))
}