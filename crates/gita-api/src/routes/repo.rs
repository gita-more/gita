//! Repository routes: create, list, get, delete, browse tree/files/commits.

use crate::error::{ApiError, ApiResult};
use crate::extract::{AuthUserExtractor, OptionalAuthUser};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use gita_core::{EventType, Repository};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct CreateRepoRequest {
    pub name: String,
    pub description: Option<String>,
    pub is_private: Option<bool>,
    pub default_branch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RepoResponse {
    pub id: i64,
    pub owner: String,
    pub name: String,
    pub full_name: String,
    pub description: String,
    pub is_private: bool,
    pub default_branch: String,
    pub created_at: String,
}

impl From<Repository> for RepoResponse {
    fn from(r: Repository) -> Self {
        let owner_name = r.owner_name.clone();
        let name = r.name.clone();
        Self {
            id: r.id,
            owner: owner_name.clone(),
            name: name.clone(),
            full_name: format!("{}/{}", owner_name, name),
            description: r.description,
            is_private: r.is_private,
            default_branch: r.default_branch,
            created_at: r.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ListReposQuery {
    pub owner: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct TreeQuery {
    pub path: Option<String>,
    pub r#ref: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CommitsQuery {
    pub r#ref: Option<String>,
    pub limit: Option<usize>,
}

pub fn repo_routes() -> Router<AppState> {
    Router::new()
        .route("/api/repos", post(create_repo).get(list_repos))
        .route("/api/repos/{owner}/{repo}", get(get_repo).delete(delete_repo))
        .route("/api/repos/{owner}/{repo}/tree", get(get_tree))
        .route("/api/repos/{owner}/{repo}/file", get(get_file))
        .route("/api/repos/{owner}/{repo}/commits", get(list_commits))
        .route("/api/repos/{owner}/{repo}/branches", get(list_branches))
}

async fn create_repo(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Json(req): Json<CreateRepoRequest>,
) -> ApiResult<Json<RepoResponse>> {
    let name = req.name.trim().to_string();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
        return Err(ApiError::BadRequest(
            "repository name may only contain letters, numbers, '-', '_', '.'".into(),
        ));
    }
    let description = req.description.unwrap_or_default();
    let is_private = req.is_private.unwrap_or(false);
    let default_branch = req.default_branch.unwrap_or_else(|| "main".to_string());

    if state
        .db
        .get_repository_by_name(&user.username, &name)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict("repository already exists".into()));
    }

    let repo = state
        .db
        .create_repository(
            user.id,
            &user.username,
            &name,
            &description,
            is_private,
            &default_branch,
        )
        .await?;

    // Create the bare repo on disk.
    state
        .git
        .create_repo(&user.username, &name, &default_branch)
        .await?;

    state.bus.publish(
        EventType::RepoCreated,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({
            "owner": user.username,
            "repo": name,
            "description": description,
        }),
    );

    Ok(Json(RepoResponse::from(repo)))
}

async fn list_repos(
    State(state): State<AppState>,
    Query(q): Query<ListReposQuery>,
) -> ApiResult<Json<Vec<RepoResponse>>> {
    let limit = q.limit.unwrap_or(100).min(500);
    let repos = state.db.list_repositories(q.owner.as_deref(), limit).await?;
    Ok(Json(repos.into_iter().map(RepoResponse::from).collect()))
}

async fn get_repo(
    State(state): State<AppState>,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<RepoResponse>> {
    let repo = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    Ok(Json(RepoResponse::from(repo)))
}

async fn delete_repo(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    if owner != user.username {
        return Err(ApiError::Forbidden("only the owner can delete this repo".into()));
    }
    let repo = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    state.db.delete_repository(repo.id).await?;
    state.git.delete_repo(&owner, &repo.name)?;
    state.bus.publish(
        EventType::RepoDeleted,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({"owner": owner, "repo": repo.name}),
    );
    Ok(Json(crate::error::MessageResponse {
        message: "repository deleted".to_string(),
    }))
}

async fn get_tree(
    State(state): State<AppState>,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<TreeQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let repo = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let reference = q.r#ref.unwrap_or_else(|| repo.default_branch.clone());
    let path = q.path.unwrap_or_default();
    let entries = state
        .git
        .list_tree(&owner, &repo.name, &reference, &path)
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "path": path,
        "ref": reference,
        "entries": entries,
    })))
}

async fn get_file(
    State(state): State<AppState>,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<TreeQuery>,
) -> ApiResult<Json<gita_core::FileContent>> {
    let repo = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let reference = q.r#ref.unwrap_or_else(|| repo.default_branch.clone());
    let path = q.path.ok_or_else(|| ApiError::BadRequest("missing 'path' query".into()))?;
    let file = state
        .git
        .read_file(&owner, &repo.name, &reference, &path)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("file not found".into()))?;
    Ok(Json(file))
}

async fn list_commits(
    State(state): State<AppState>,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<CommitsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let repo = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let reference = q.r#ref.unwrap_or_else(|| repo.default_branch.clone());
    let limit = q.limit.unwrap_or(30).min(200);
    let commits = state
        .git
        .list_commits(&owner, &repo.name, &reference, limit)
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "ref": reference,
        "commits": commits,
    })))
}

async fn list_branches(
    State(state): State<AppState>,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<Vec<String>>> {
    let repo = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let branches = state
        .git
        .list_branches(&owner, &repo.name)
        .map_err(ApiError::from)?;
    Ok(Json(branches))
}