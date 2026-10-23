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
        // The caller must be allowed to see the repository. Without this
        // check any registered user could read the event stream of a
        // private repository — which leaks who pushed what, which issues
        // were opened, and so on.
        let repo =
            crate::routes::visibility::load_readable_repo(&state, owner, name, Some(&user)).await?;
        Some(repo.id)
    } else {
        None
    };
    let limit = q.limit.unwrap_or(50).min(500);
    let mut events = state
        .db
        .list_events(repo_id, q.r#type.as_deref(), limit)
        .await?;
    // Without a repository filter the query returns the global event log,
    // which includes events belonging to private repositories the caller
    // cannot see. Drop those.
    if repo_id.is_none() {
        let visible = state
            .db
            .filter_readable(state.db.list_repositories(None, 5000).await?, Some(user.id))
            .await?
            .into_iter()
            .map(|r| r.id)
            .collect::<std::collections::HashSet<i64>>();
        events.retain(|e| match e.repo_id {
            Some(id) => visible.contains(&id),
            // Repository-less events (e.g. user registration) are visible.
            None => true,
        });
    }
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
        "git": gita_git::git_version(),
    })))
}

async fn list_mcp_tools(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let _ = state;
    Ok(Json(serde_json::json!({
        "tools": [
            {"name": "list_repositories", "description": "List all repositories"},
            {"name": "read_file", "description": "Read a file from a repository"},
            {"name": "list_commits", "description": "List commits of a repository"},
            {"name": "create_pull_request", "description": "Create a pull request"},
            {"name": "get_pull_request", "description": "Get a pull request"},
            {"name": "create_issue", "description": "Create an issue"},
            {"name": "list_events", "description": "List AI-readable events"}
        ]
    })))
}
