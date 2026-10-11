//! Semantic search routes.

use crate::error::{ApiError, ApiResult};
use crate::extract::AuthUserExtractor;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    pub repo: Option<String>,
    pub limit: Option<usize>,
}

pub fn search_routes() -> Router<AppState> {
    Router::new().route("/api/search", get(search))
}

async fn search(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let query = q.q.trim().to_string();
    if query.is_empty() {
        return Err(ApiError::BadRequest("missing 'q' query parameter".into()));
    }
    let limit = q.limit.unwrap_or(20).min(100);

    // Resolve repo_id if a repo is specified.
    let repo_id = if let Some(full_name) = &q.repo {
        let (owner, name) = full_name
            .split_once('/')
            .ok_or_else(|| ApiError::BadRequest("repo must be in owner/name format".into()))?;
        let repo = state
            .db
            .get_repository_by_name(owner, name)
            .await?
            .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
        Some(repo.id)
    } else {
        None
    };

    let hits = state.search.search(&query, repo_id, limit).await?;

    Ok(Json(serde_json::json!({
        "query": query,
        "hits": hits,
    })))
}