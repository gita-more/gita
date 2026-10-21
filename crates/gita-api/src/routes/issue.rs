//! Issue routes: create, list, get, close.

use crate::error::{ApiError, ApiResult};
use crate::extract::{AuthUserExtractor, OptionalAuthUser};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use gita_core::{EventType, IssueStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct CreateIssueRequest {
    pub title: String,
    pub body: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListIssuesQuery {
    pub status: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct IssueResponse {
    pub id: i64,
    pub number: i64,
    pub title: String,
    pub body: String,
    pub status: String,
    pub author: String,
    pub created_at: String,
}

impl From<gita_core::Issue> for IssueResponse {
    fn from(i: gita_core::Issue) -> Self {
        Self {
            id: i.id,
            number: i.number,
            title: i.title,
            body: i.body,
            status: i.status.to_string(),
            author: i.author_name,
            created_at: i.created_at.to_rfc3339(),
        }
    }
}

pub fn issue_routes() -> Router<AppState> {
    Router::new()
        .route("/api/repos/{owner}/{repo}/issues", post(create_issue).get(list_issues))
        .route("/api/repos/{owner}/{repo}/issues/{number}", get(get_issue))
        .route("/api/repos/{owner}/{repo}/issues/{number}/close", post(close_issue))
        .route("/api/issues", get(list_global_issues))
}

/// Global issue list across all repositories.
async fn list_global_issues(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Query(q): Query<ListIssuesQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let status = match q.status.as_deref() {
        None | Some("open") => Some(IssueStatus::Open),
        Some("closed") => Some(IssueStatus::Closed),
        Some("all") => None,
        Some(_) => return Err(ApiError::BadRequest("invalid status".into())),
    };
    let limit = q.limit.unwrap_or(50).min(200);
    let items = state.db.list_all_issues(status, limit).await?;
    Ok(Json(serde_json::json!({
        "issues": items
            .into_iter()
            // Skip issues belonging to private repositories the caller
            // cannot see; the list spans every repository.
            .filter(|(_, repo)| {
                crate::routes::visibility::can_read(repo, user.0.as_ref())
            })
            .map(|(issue, repo)| serde_json::json!({
                "id": issue.id,
                "number": issue.number,
                "title": issue.title,
                "body": issue.body,
                "status": issue.status.to_string(),
                "author": issue.author_name,
                "repo_owner": repo.owner_name,
                "repo_name": repo.name,
                "full_name": format!("{}/{}", repo.owner_name, repo.name),
                "created_at": issue.created_at.to_rfc3339(),
            }))
            .collect::<Vec<_>>(),
    })))
}

async fn create_issue(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo)): Path<(String, String)>,
    Json(req): Json<CreateIssueRequest>,
) -> ApiResult<Json<IssueResponse>> {
    let repo = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let title = req.title.trim().to_string();
    if title.is_empty() {
        return Err(ApiError::BadRequest("title is required".into()));
    }
    let issue = state
        .db
        .create_issue(repo.id, &title, req.body.as_deref().unwrap_or(""), user.id, &user.username)
        .await?;

    state.bus.publish(
        EventType::IssueCreated,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({"issue": issue.number, "title": issue.title}),
    );

    Ok(Json(IssueResponse::from(issue)))
}

async fn list_issues(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<ListIssuesQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let repo = crate::routes::visibility::load_readable_repo(
        &state,
        &owner,
        &repo,
        user.0.as_ref(),
    )
    .await?;
    let status = match q.status.as_deref() {
        None | Some("open") => Some(IssueStatus::Open),
        Some("closed") => Some(IssueStatus::Closed),
        Some("all") => None,
        Some(_) => return Err(ApiError::BadRequest("invalid status".into())),
    };
    let limit = q.limit.unwrap_or(30).min(200);
    let issues = state.db.list_issues(repo.id, status, limit).await?;
    Ok(Json(serde_json::json!({
        "issues": issues.into_iter().map(IssueResponse::from).collect::<Vec<_>>(),
    })))
}

async fn get_issue(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Json<IssueResponse>> {
    let repo = crate::routes::visibility::load_readable_repo(
        &state,
        &owner,
        &repo,
        user.0.as_ref(),
    )
    .await?;
    let issue = state
        .db
        .get_issue(repo.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("issue not found".into()))?;
    Ok(Json(IssueResponse::from(issue)))
}

async fn close_issue(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let repo = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let issue = state
        .db
        .get_issue(repo.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("issue not found".into()))?;
    if issue.status == IssueStatus::Closed {
        return Err(ApiError::Conflict("issue already closed".into()));
    }
    state.db.update_issue_status(issue.id, IssueStatus::Closed).await?;
    state.bus.publish(
        EventType::IssueClosed,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({"issue": issue.number, "title": issue.title}),
    );
    Ok(Json(crate::error::MessageResponse {
        message: "issue closed".to_string(),
    }))
}