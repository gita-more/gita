//! Comment routes: create/list/delete comments on pull requests and issues.

use crate::error::{ApiError, ApiResult, MessageResponse};
use crate::extract::{AuthUserExtractor, OptionalAuthUser};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use gita_core::{Comment, EventType};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct CreateCommentRequest {
    pub body: String,
}

#[derive(Debug, Serialize)]
pub struct CommentResponse {
    pub id: i64,
    pub repo_id: i64,
    pub pr_id: Option<i64>,
    pub issue_id: Option<i64>,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

impl From<Comment> for CommentResponse {
    fn from(c: Comment) -> Self {
        Self {
            id: c.id,
            repo_id: c.repo_id,
            pr_id: c.pr_id,
            issue_id: c.issue_id,
            author: c.author_name,
            body: c.body,
            created_at: c.created_at.to_rfc3339(),
        }
    }
}

pub fn comment_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/repos/{owner}/{repo}/prs/{number}/comments",
            get(list_pr_comments).post(create_pr_comment),
        )
        .route(
            "/api/repos/{owner}/{repo}/issues/{number}/comments",
            get(list_issue_comments).post(create_issue_comment),
        )
        .route("/api/comments/{id}", axum::routing::delete(delete_comment))
}

async fn create_pr_comment(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Json(req): Json<CreateCommentRequest>,
) -> ApiResult<Json<CommentResponse>> {
    let body = req.body.trim().to_string();
    if body.is_empty() {
        return Err(ApiError::BadRequest("comment body is required".into()));
    }

    let repository =
        crate::routes::visibility::load_writable_repo(&state, &owner, &repo, &user).await?;
    let pr = state
        .db
        .get_pull_request(repository.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("pull request not found".into()))?;

    let comment = state
        .db
        .create_pr_comment(repository.id, pr.id, user.id, &user.username, &body)
        .await?;

    state.bus.publish(
        EventType::CommentCreated,
        Some(repository.id),
        Some(user.id),
        serde_json::json!({
            "pr": pr.number,
            "comment_id": comment.id,
            "repo": repository.name,
        }),
    );

    Ok(Json(CommentResponse::from(comment)))
}

async fn list_pr_comments(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository =
        crate::routes::visibility::load_readable_repo(&state, &owner, &repo, user.0.as_ref())
            .await?;
    let pr = state
        .db
        .get_pull_request(repository.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("pull request not found".into()))?;

    let comments = state.db.list_pr_comments(pr.id, 500).await?;
    Ok(Json(serde_json::json!({
        "comments": comments
            .into_iter()
            .map(CommentResponse::from)
            .collect::<Vec<_>>(),
    })))
}

async fn create_issue_comment(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Json(req): Json<CreateCommentRequest>,
) -> ApiResult<Json<CommentResponse>> {
    let body = req.body.trim().to_string();
    if body.is_empty() {
        return Err(ApiError::BadRequest("comment body is required".into()));
    }

    let repository =
        crate::routes::visibility::load_writable_repo(&state, &owner, &repo, &user).await?;
    let issue = state
        .db
        .get_issue(repository.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("issue not found".into()))?;

    let comment = state
        .db
        .create_issue_comment(repository.id, issue.id, user.id, &user.username, &body)
        .await?;

    state.bus.publish(
        EventType::CommentCreated,
        Some(repository.id),
        Some(user.id),
        serde_json::json!({
            "issue": issue.number,
            "comment_id": comment.id,
            "repo": repository.name,
        }),
    );

    Ok(Json(CommentResponse::from(comment)))
}

async fn list_issue_comments(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Json<serde_json::Value>> {
    let repository =
        crate::routes::visibility::load_readable_repo(&state, &owner, &repo, user.0.as_ref())
            .await?;
    let issue = state
        .db
        .get_issue(repository.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("issue not found".into()))?;

    let comments = state.db.list_issue_comments(issue.id, 500).await?;
    Ok(Json(serde_json::json!({
        "comments": comments
            .into_iter()
            .map(CommentResponse::from)
            .collect::<Vec<_>>(),
    })))
}

async fn delete_comment(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(id): Path<i64>,
) -> ApiResult<Json<MessageResponse>> {
    let comment = state
        .db
        .get_comment(id)
        .await?
        .ok_or_else(|| ApiError::NotFound("comment not found".into()))?;

    // Only the author or someone who may administer the repository may
    // delete a comment. The repository's own access rule decides the
    // second half of that.
    let repo = state
        .db
        .get_repository(comment.repo_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if comment.author_id != user.id {
        let may_administer = state
            .db
            .can_access(&repo, Some(user.id), gita_core::access::Access::Admin)
            .await?;
        if !may_administer {
            return Err(ApiError::NotFound("comment not found".into()));
        }
    }

    state.db.delete_comment(id).await?;

    state.bus.publish(
        EventType::CommentDeleted,
        Some(comment.repo_id),
        Some(user.id),
        serde_json::json!({
            "comment_id": id,
        }),
    );

    Ok(Json(MessageResponse {
        message: "comment deleted".to_string(),
    }))
}
