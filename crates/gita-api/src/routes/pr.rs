//! Pull request routes: create, list, get, merge, close, review.

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
        }
    }
}

pub fn pr_routes() -> Router<AppState> {
    Router::new()
        .route("/api/repos/{owner}/{repo}/prs", post(create_pr).get(list_prs))
        .route("/api/repos/{owner}/{repo}/prs/{number}", get(get_pr))
        .route("/api/repos/{owner}/{repo}/prs/{number}/merge", post(merge_pr))
        .route("/api/repos/{owner}/{repo}/prs/{number}/close", post(close_pr))
        .route("/api/repos/{owner}/{repo}/prs/{number}/review", post(trigger_review))
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
    let repo = state
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
    let prs = state.db.list_pull_requests(repo.id, status, limit).await?;
    Ok(Json(serde_json::json!({
        "prs": prs.into_iter().map(PrResponse::from).collect::<Vec<_>>(),
    })))
}

async fn get_pr(
    State(state): State<AppState>,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Json<serde_json::Value>> {
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
    // Include AI reviews for this PR.
    let reviews = state.db.list_reviews(None, Some(pr.id), 10).await?;
    Ok(Json(serde_json::json!({
        "pr": PrResponse::from(pr),
        "reviews": reviews,
    })))
}

async fn merge_pr(
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

    // Perform the merge: fast-forward target branch to head SHA.
    let head_sha = pr.head_sha.clone();
    state
        .git
        .merge_branch(&owner, &repo.name, &pr.target_branch, &head_sha)
        .map_err(ApiError::from)?;

    state
        .db
        .update_pr_status(pr.id, PrStatus::Merged, Some(chrono::Utc::now()))
        .await?;

    state.bus.publish(
        EventType::PullRequestMerged,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({
            "pr": pr.number,
            "title": pr.title,
        }),
    );

    Ok(Json(crate::error::MessageResponse {
        message: "pull request merged".to_string(),
    }))
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

async fn trigger_review(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo_name, number)): Path<(String, String, i64)>,
) -> ApiResult<Json<gita_core::Review>> {
    let repo = state
        .db
        .get_repository_by_name(&owner, &repo_name)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let pr = state
        .db
        .get_pull_request(repo.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("pull request not found".into()))?;

    // Compute the diff between base and head.
    let diff = state
        .git
        .diff_between(&owner, &repo_name, &pr.target_branch, &pr.source_branch)
        .map_err(ApiError::from)?;

    let result = state
        .ai
        .review_diff(&diff, &format!("{owner}/{repo_name}"))
        .await
        .map_err(ApiError::from)?;

    let review = gita_ai::review_from_result(
        repo.id,
        Some(pr.id),
        &pr.head_sha,
        result,
    );

    let review_id = state.db.insert_review(&review).await?;
    let mut saved = state
        .db
        .get_review(review_id)
        .await?
        .ok_or_else(|| ApiError::Internal("review not found after insert".into()))?;
    saved.id = review_id;

    state.bus.publish(
        EventType::ReviewCompleted,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({
            "pr": pr.number,
            "review_id": review_id,
            "risk_score": saved.risk_score,
        }),
    );

    Ok(Json(saved))
}

// Review listing routes are in review.rs