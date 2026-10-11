//! Event stream and misc routes.

use crate::error::{ApiError, ApiResult};
use crate::extract::AuthUserExtractor;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub repo: Option<String>,
    pub r#type: Option<String>,
    pub limit: Option<usize>,
}

pub fn event_routes() -> Router<AppState> {
    Router::new().route("/api/events", get(list_events))
}

async fn list_events(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Query(q): Query<EventsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
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
    let limit = q.limit.unwrap_or(50).min(500);
    let events = state
        .db
        .list_events(repo_id, q.r#type.as_deref(), limit)
        .await?;
    Ok(Json(serde_json::json!({
        "events": events,
    })))
}

// ---------------------------------------------------------------------------
// Misc routes (health, mcp bridge)
// ---------------------------------------------------------------------------

pub fn misc_routes() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/mcp/tools", get(list_mcp_tools))
}

async fn health(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    // Check DB connectivity.
    let db_ok = sqlx::query("SELECT 1")
        .fetch_one(state.db.pool())
        .await
        .is_ok();
    Ok(Json(serde_json::json!({
        "status": if db_ok { "ok" } else { "degraded" },
        "db": if db_ok { "ok" } else { "error" },
        "ai_configured": state.ai.configured,
        "git": gita_git::git_version(),
    })))
}

async fn list_mcp_tools(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let _ = state;
    Ok(Json(serde_json::json!({
        "tools": [
            {"name": "list_repositories", "description": "List all repositories"},
            {"name": "read_file", "description": "Read a file from a repository"},
            {"name": "search_code", "description": "Semantic code search"},
            {"name": "list_commits", "description": "List commits of a repository"},
            {"name": "create_pull_request", "description": "Create a pull request"},
            {"name": "get_pull_request", "description": "Get a pull request"},
            {"name": "get_review", "description": "Get an AI review"},
            {"name": "create_issue", "description": "Create an issue"},
            {"name": "list_events", "description": "List AI-readable events"}
        ]
    })))
}