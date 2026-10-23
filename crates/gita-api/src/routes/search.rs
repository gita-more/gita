//! Search routes: repository search (DB) and code search (git grep).

use crate::error::{ApiError, ApiResult};
use crate::extract::OptionalAuthUser;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use gita_core::Repository;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    pub r#type: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct RepoSearchHit {
    pub id: i64,
    pub owner: String,
    pub name: String,
    pub full_name: String,
    pub description: String,
    pub is_private: bool,
}

#[derive(Debug, Serialize)]
pub struct CodeSearchHit {
    pub owner: String,
    pub repo: String,
    pub path: String,
    pub line: i64,
    pub content: String,
}

impl From<Repository> for RepoSearchHit {
    fn from(r: Repository) -> Self {
        Self {
            id: r.id,
            owner: r.owner_name.clone(),
            name: r.name.clone(),
            full_name: r.full_name(),
            description: r.description,
            is_private: r.is_private,
        }
    }
}

pub fn search_routes() -> Router<AppState> {
    Router::new()
        .route("/api/search/repos", get(search_repos))
        .route("/api/search/code", get(search_code))
        .route("/api/search", get(search_all))
}

/// The repositories whose contents the caller is allowed to read.
///
/// Code search walks the bare repositories on disk, so without this filter
/// an anonymous caller would be able to search — and read the matching
/// lines of — a *private* repository's files.
async fn readable_repos(
    state: &AppState,
    user: Option<&crate::extract::AuthUserExtractor>,
) -> std::collections::HashSet<(String, String)> {
    state
        .db
        .filter_readable(
            state
                .db
                .list_repositories(None, 5000)
                .await
                .unwrap_or_default(),
            user.map(|u| u.id),
        )
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.owner_name, r.name))
        .collect()
}

async fn search_repos(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let query = q.q.trim().to_string();
    if query.is_empty() {
        return Err(ApiError::BadRequest("query 'q' is required".into()));
    }
    let limit = q.limit.unwrap_or(50).min(200);
    let hits = state.db.search_repositories(&query, limit).await?;
    Ok(Json(serde_json::json!({
        "hits": hits.into_iter().map(RepoSearchHit::from).collect::<Vec<_>>(),
    })))
}

async fn search_code(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let query = q.q.trim().to_string();
    if query.is_empty() {
        return Err(ApiError::BadRequest("query 'q' is required".into()));
    }
    let limit = q.limit.unwrap_or(50).min(200);
    let allowed = readable_repos(&state, user.0.as_ref()).await;
    let hits = state
        .git
        .search_code(&query, limit, Some(&allowed))
        .map_err(ApiError::from)?;
    let code: Vec<CodeSearchHit> = hits
        .into_iter()
        .map(|(owner, repo, path, line, content)| CodeSearchHit {
            owner,
            repo,
            path,
            line,
            content,
        })
        .collect();
    Ok(Json(serde_json::json!({
        "hits": code,
    })))
}

async fn search_all(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let query = q.q.trim().to_string();
    if query.is_empty() {
        return Err(ApiError::BadRequest("query 'q' is required".into()));
    }
    let limit = q.limit.unwrap_or(20).min(100);
    let r#type = q.r#type.as_deref().unwrap_or("all");
    let mut out = serde_json::json!({});
    if matches!(r#type, "all" | "repos") {
        let repos = state.db.search_repositories(&query, limit).await?;
        let repo_hits: Vec<RepoSearchHit> = repos.into_iter().map(Into::into).collect();
        out["repos"] = serde_json::json!(repo_hits);
    }
    if matches!(r#type, "all" | "code") {
        let allowed = readable_repos(&state, user.0.as_ref()).await;
        let hits = state
            .git
            .search_code(&query, limit, Some(&allowed))
            .map_err(ApiError::from)?;
        let code: Vec<CodeSearchHit> = hits
            .into_iter()
            .map(|(owner, repo, path, line, content)| CodeSearchHit {
                owner,
                repo,
                path,
                line,
                content,
            })
            .collect();
        out["code"] = serde_json::json!(code);
    }
    Ok(Json(out))
}
