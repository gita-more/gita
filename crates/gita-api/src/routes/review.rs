//! Review routes: list reviews, get a single review, user discussion
//! comments, and review modification (summary / risk_score / resolution).

use crate::error::{ApiError, ApiResult, MessageResponse};
use crate::extract::AuthUserExtractor;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use gita_core::{EventType, ReviewDiscussionComment};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ListReviewsQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct CreateDiscussionCommentRequest {
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateDiscussionCommentRequest {
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateReviewRequest {
    pub summary: Option<String>,
    pub risk_score: Option<i64>,
}

pub fn review_routes() -> Router<AppState> {
    Router::new()
        .route("/api/reviews", get(list_reviews))
        .route("/api/reviews/{id}", get(get_review).patch(update_review))
        .route(
            "/api/reviews/{id}/comments",
            get(list_discussion_comments).post(create_discussion_comment),
        )
        .route(
            "/api/reviews/comments/{comment_id}",
            patch(update_discussion_comment).delete(delete_discussion_comment),
        )
        .route(
            "/api/reviews/comments/{comment_id}/resolve",
            post(resolve_comment),
        )
        .route(
            "/api/reviews/comments/{comment_id}/unresolve",
            post(unresolve_comment),
        )
}

async fn list_reviews(
    State(state): State<AppState>,
    Query(q): Query<ListReviewsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(30).min(200);
    let reviews = state.db.list_reviews(None, None, limit).await?;
    Ok(Json(serde_json::json!({
        "reviews": reviews,
    })))
}

async fn get_review(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<gita_core::Review>> {
    let review = state
        .db
        .get_review(id)
        .await?
        .ok_or_else(|| ApiError::NotFound("review not found".into()))?;
    Ok(Json(review))
}

/// PATCH /api/reviews/{id} — edit the review summary / risk score.
/// Only the repository owner may modify a review.
async fn update_review(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(id): Path<i64>,
    Json(req): Json<UpdateReviewRequest>,
) -> ApiResult<Json<gita_core::Review>> {
    let review = state
        .db
        .get_review(id)
        .await?
        .ok_or_else(|| ApiError::NotFound("review not found".into()))?;

    // The repository owner may edit the review.
    let repo = state
        .db
        .get_repository(review.repo_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if repo.owner_id != user.id {
        return Err(ApiError::Forbidden(
            "only the repository owner can modify a review".into(),
        ));
    }

    // Validate risk_score range if provided.
    if let Some(score) = req.risk_score {
        if !(0..=100).contains(&score) {
            return Err(ApiError::BadRequest(
                "risk_score must be between 0 and 100".into(),
            ));
        }
    }

    let updated = state
        .db
        .update_review(
            id,
            req.summary.as_deref(),
            req.risk_score,
        )
        .await?;
    if !updated {
        return Err(ApiError::NotFound("review not found".into()));
    }

    let saved = state
        .db
        .get_review(id)
        .await?
        .ok_or_else(|| ApiError::Internal("review not found after update".into()))?;
    Ok(Json(saved))
}

/// POST /api/reviews/{id}/comments — post a user comment on a review.
async fn create_discussion_comment(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(id): Path<i64>,
    Json(req): Json<CreateDiscussionCommentRequest>,
) -> ApiResult<Json<ReviewDiscussionComment>> {
    let review = state
        .db
        .get_review(id)
        .await?
        .ok_or_else(|| ApiError::NotFound("review not found".into()))?;

    let body = req.body.trim().to_string();
    if body.is_empty() {
        return Err(ApiError::BadRequest("body is required".into()));
    }

    let comment = state
        .db
        .insert_review_discussion_comment(id, user.id, &user.username, &body)
        .await?;

    state.bus.publish(
        EventType::CommentAdded,
        Some(review.repo_id),
        Some(user.id),
        serde_json::json!({
            "review_id": id,
            "comment_id": comment.id,
            "author": user.username,
        }),
    );

    Ok(Json(comment))
}

/// GET /api/reviews/{id}/comments — list user comments on a review.
async fn list_discussion_comments(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let review = state
        .db
        .get_review(id)
        .await?
        .ok_or_else(|| ApiError::NotFound("review not found".into()))?;
    let comments = state
        .db
        .list_review_discussion_comments(id)
        .await?;
    Ok(Json(serde_json::json!({
        "review_id": review.id,
        "comments": comments,
    })))
}

/// PATCH /api/reviews/comments/{comment_id} — edit your own comment.
async fn update_discussion_comment(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(comment_id): Path<i64>,
    Json(req): Json<UpdateDiscussionCommentRequest>,
) -> ApiResult<Json<ReviewDiscussionComment>> {
    let comment = state
        .db
        .get_review_discussion_comment(comment_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("comment not found".into()))?;
    if comment.author_id != user.id {
        return Err(ApiError::Forbidden(
            "only the comment author can edit it".into(),
        ));
    }
    let body = req.body.trim().to_string();
    if body.is_empty() {
        return Err(ApiError::BadRequest("body is required".into()));
    }
    let updated = state
        .db
        .update_review_discussion_comment(comment_id, user.id, &body)
        .await?;
    if !updated {
        return Err(ApiError::NotFound("comment not found".into()));
    }
    let saved = state
        .db
        .get_review_discussion_comment(comment_id)
        .await?
        .ok_or_else(|| ApiError::Internal("comment not found after update".into()))?;
    Ok(Json(saved))
}

/// DELETE /api/reviews/comments/{comment_id} — soft-delete your own comment.
async fn delete_discussion_comment(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(comment_id): Path<i64>,
) -> ApiResult<Json<MessageResponse>> {
    let comment = state
        .db
        .get_review_discussion_comment(comment_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("comment not found".into()))?;
    if comment.author_id != user.id {
        return Err(ApiError::Forbidden(
            "only the comment author can delete it".into(),
        ));
    }
    let deleted = state
        .db
        .delete_review_discussion_comment(comment_id, user.id)
        .await?;
    if !deleted {
        return Err(ApiError::NotFound("comment not found".into()));
    }
    Ok(Json(MessageResponse {
        message: "comment deleted".to_string(),
    }))
}

/// POST /api/reviews/comments/{comment_id}/resolve — mark an AI review
/// comment as resolved. Only the repository owner can do this.
async fn resolve_comment(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(comment_id): Path<i64>,
) -> ApiResult<Json<MessageResponse>> {
    set_resolved(&state, &user, comment_id, true).await
}

/// POST /api/reviews/comments/{comment_id}/unresolve — clear the resolved
/// flag on an AI review comment. Only the repository owner can do this.
async fn unresolve_comment(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(comment_id): Path<i64>,
) -> ApiResult<Json<MessageResponse>> {
    set_resolved(&state, &user, comment_id, false).await
}

async fn set_resolved(
    state: &AppState,
    user: &AuthUserExtractor,
    comment_id: i64,
    resolved: bool,
) -> ApiResult<Json<MessageResponse>> {
    // Fetch the AI review comment to determine which review it belongs to.
    let comment = state
        .db
        .get_review_comment(comment_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("comment not found".into()))?;

    // Fetch the review to determine the repository.
    let review = state
        .db
        .get_review(comment.review_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("review not found".into()))?;

    // Only the repository owner may resolve/unresolve comments.
    let repo = state
        .db
        .get_repository(review.repo_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if repo.owner_id != user.id {
        return Err(ApiError::Forbidden(
            "only the repository owner can resolve review comments".into(),
        ));
    }

    let action = if resolved { "resolve" } else { "unresolve" };
    let updated = state
        .db
        .set_review_comment_resolved(comment_id, resolved, user.id)
        .await?;
    if !updated {
        return Err(ApiError::NotFound("comment not found".into()));
    }

    state.bus.publish(
        EventType::CommentAdded,
        Some(review.repo_id),
        Some(user.id),
        serde_json::json!({
            "review_id": review.id,
            "comment_id": comment_id,
            "action": action,
            "author": user.username,
        }),
    );

    Ok(Json(MessageResponse {
        message: format!("comment {action}d"),
    }))
}