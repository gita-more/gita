//! Review routes: list reviews, get a single review.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ListReviewsQuery {
    pub limit: Option<usize>,
}

pub fn review_routes() -> Router<AppState> {
    Router::new()
        .route("/api/reviews", get(list_reviews))
        .route("/api/reviews/{id}", get(get_review))
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