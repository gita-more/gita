//! Issue handlers: create, list, get, close.
//!
//! Repository-scoped issues are served through the single catch-all route
//! in [`crate::routes::repo`]; the two `*_dispatch` functions below are
//! called from that dispatcher once the path's `issues` marker segment has
//! been split off. `rest` is everything after the marker. The global
//! `/api/issues` list keeps its own route.

use crate::error::{ApiError, ApiResult};
use crate::extract::{AuthUserExtractor, OptionalAuthUser};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::{Json, Router};
use gita_core::{EventType, IssueStatus};
use serde::{Deserialize, Serialize};

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
    Router::new().route("/api/issues", axum::routing::get(list_global_issues))
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
    // Skip issues belonging to private repositories the caller cannot see;
    // the list spans every repository. The caller's group memberships are
    // resolved once rather than per issue.
    let repos: Vec<gita_core::Repository> = items.iter().map(|(_, r)| r.clone()).collect();
    let readable_ids: std::collections::HashSet<i64> = state
        .db
        .filter_readable(repos, user.0.as_ref().map(|u| u.id))
        .await?
        .into_iter()
        .map(|r| r.id)
        .collect();
    let items: Vec<_> = items
        .into_iter()
        .filter(|(_, repo)| readable_ids.contains(&repo.id))
        .collect();
    Ok(Json(serde_json::json!({
        "issues": items
            .into_iter()
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

/// GET dispatch for `/api/repos/{owner}/{repo}/issues[...]`.
///
/// `rest` is the path after the `issues` marker: `""` (list), `{number}`,
/// `{number}/comments`.
pub(crate) async fn get_dispatch(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
    rest: &str,
    q: &std::collections::HashMap<String, String>,
) -> ApiResult<axum::response::Response> {
    let segs: Vec<&str> = rest.split('/').collect();
    match segs.as_slice() {
        [""] => {
            let res = list_issues(
                state,
                user,
                owner,
                repo,
                q.get("status").cloned(),
                q.get("limit").and_then(|v| v.parse::<usize>().ok()),
            )
            .await?;
            Ok(res.into_response())
        }
        [number] => {
            let number = parse_number(number)?;
            let res = get_issue(state, user, owner, repo, number).await?;
            Ok(res.into_response())
        }
        [number, "comments"] => {
            let number = parse_number(number)?;
            let res =
                crate::routes::comment::list_issue_comments(state, user, owner, repo, number)
                    .await?;
            Ok(res.into_response())
        }
        _ => Err(ApiError::NotFound("endpoint not found".into())),
    }
}

/// POST dispatch for `/api/repos/{owner}/{repo}/issues[...]`.
///
/// `rest` is the path after the `issues` marker: `""` (create),
/// `{number}/close`, `{number}/comments`.
pub(crate) async fn post_dispatch(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    rest: &str,
    body: &[u8],
) -> ApiResult<axum::response::Response> {
    let segs: Vec<&str> = rest.split('/').collect();
    match segs.as_slice() {
        [""] => {
            let req: CreateIssueRequest = serde_json::from_slice(body)
                .map_err(|e| ApiError::BadRequest(format!("invalid request body: {e}")))?;
            let res = create_issue(state, user, owner, repo, req).await?;
            Ok(res.into_response())
        }
        [number, "close"] => {
            let number = parse_number(number)?;
            let res = close_issue(state, user, owner, repo, number).await?;
            Ok(res.into_response())
        }
        [number, "comments"] => {
            let number = parse_number(number)?;
            let req: crate::routes::comment::CreateCommentRequest = serde_json::from_slice(body)
                .map_err(|e| ApiError::BadRequest(format!("invalid request body: {e}")))?;
            let res =
                crate::routes::comment::create_issue_comment(state, user, owner, repo, number, req)
                    .await?;
            Ok(res.into_response())
        }
        _ => Err(ApiError::NotFound("endpoint not found".into())),
    }
}

fn parse_number(s: &str) -> ApiResult<i64> {
    s.parse()
        .map_err(|_| ApiError::BadRequest("invalid issue number".into()))
}

#[derive(Debug, Deserialize)]
pub struct CreateIssueRequest {
    pub title: String,
    pub body: Option<String>,
}

async fn create_issue(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    req: CreateIssueRequest,
) -> ApiResult<Json<IssueResponse>> {
    let repo = crate::routes::visibility::load_writable_repo(state, owner, repo, user).await?;
    let title = req.title.trim().to_string();
    if title.is_empty() {
        return Err(ApiError::BadRequest("title is required".into()));
    }
    let issue = state
        .db
        .create_issue(
            repo.id,
            &title,
            req.body.as_deref().unwrap_or(""),
            user.id,
            &user.username,
        )
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
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
    status: Option<String>,
    limit: Option<usize>,
) -> ApiResult<Json<serde_json::Value>> {
    let repo =
        crate::routes::visibility::load_readable_repo(state, owner, repo, user.0.as_ref()).await?;
    let status = match status.as_deref() {
        None | Some("open") => Some(IssueStatus::Open),
        Some("closed") => Some(IssueStatus::Closed),
        Some("all") => None,
        Some(_) => return Err(ApiError::BadRequest("invalid status".into())),
    };
    let limit = limit.unwrap_or(30).min(200);
    let issues = state.db.list_issues(repo.id, status, limit).await?;
    Ok(Json(serde_json::json!({
        "issues": issues.into_iter().map(IssueResponse::from).collect::<Vec<_>>(),
    })))
}

async fn get_issue(
    state: &AppState,
    user: &OptionalAuthUser,
    owner: &str,
    repo: &str,
    number: i64,
) -> ApiResult<Json<IssueResponse>> {
    let repo =
        crate::routes::visibility::load_readable_repo(state, owner, repo, user.0.as_ref()).await?;
    let issue = state
        .db
        .get_issue(repo.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("issue not found".into()))?;
    Ok(Json(IssueResponse::from(issue)))
}

async fn close_issue(
    state: &AppState,
    user: &AuthUserExtractor,
    owner: &str,
    repo: &str,
    number: i64,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let repo = crate::routes::visibility::load_writable_repo(state, owner, repo, user).await?;
    let issue = state
        .db
        .get_issue(repo.id, number)
        .await?
        .ok_or_else(|| ApiError::NotFound("issue not found".into()))?;
    if issue.status == IssueStatus::Closed {
        return Err(ApiError::Conflict("issue already closed".into()));
    }
    state
        .db
        .update_issue_status(issue.id, IssueStatus::Closed)
        .await?;
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
