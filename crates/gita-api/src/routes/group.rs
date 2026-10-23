//! Group routes: create, list, get group details + its repositories,
//! and create a repository inside a group.

use crate::error::{ApiError, ApiResult};
use crate::extract::{AuthUserExtractor, OptionalAuthUser};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use gita_core::EventType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct CreateGroupRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GroupResponse {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub owner_id: i64,
    pub created_at: String,
}

impl From<gita_core::Group> for GroupResponse {
    fn from(g: gita_core::Group) -> Self {
        Self {
            id: g.id,
            name: g.name,
            description: g.description,
            owner_id: g.owner_id,
            created_at: g.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateGroupRepoRequest {
    pub name: String,
    pub description: Option<String>,
    pub is_private: Option<bool>,
    pub default_branch: Option<String>,
}

pub fn group_routes() -> Router<AppState> {
    Router::new()
        .route("/api/groups", axum::routing::post(create_group).get(list_groups))
        .route("/api/groups/{group}", get(get_group))
        .route("/api/groups/{group}/repos", axum::routing::post(create_group_repo))
        .route(
            "/api/groups/{group}/members",
            get(list_group_members).post(add_group_member),
        )
        .route(
            "/api/groups/{group}/members/{username}",
            axum::routing::delete(remove_group_member).patch(update_group_member),
        )
}

#[derive(Debug, Deserialize)]
pub struct AddGroupMemberRequest {
    pub username: String,
    pub role: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateGroupMemberRequest {
    pub role: String,
}

async fn create_group(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Json(req): Json<CreateGroupRequest>,
) -> ApiResult<Json<GroupResponse>> {
    let name = req.name.trim().to_string();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
        return Err(ApiError::BadRequest(
            "group name may only contain letters, numbers, '-', '_', '.'".into(),
        ));
    }
    let description = req.description.unwrap_or_default();

    if state.db.get_group_by_name(&name).await?.is_some() {
        return Err(ApiError::Conflict("group already exists".into()));
    }

    let group = state.db.create_group(&name, &description, user.id).await?;

    state.bus.publish(
        EventType::RepoCreated,
        None,
        Some(user.id),
        serde_json::json!({"group": name}),
    );

    Ok(Json(GroupResponse::from(group)))
}

async fn list_groups(
    State(state): State<AppState>,
    user: AuthUserExtractor,
) -> ApiResult<Json<Vec<GroupResponse>>> {
    let groups = state.db.list_groups_for_user(user.id).await?;
    Ok(Json(groups.into_iter().map(GroupResponse::from).collect()))
}

async fn get_group(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path(group): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let g = state
        .db
        .get_group_by_name(&group)
        .await?
        .ok_or_else(|| ApiError::NotFound("group not found".into()))?;
    let repos = state
        .db
        .filter_readable(state.db.list_group_repositories(g.id).await?, user.0.as_ref().map(|u| u.id))
        .await?;
    Ok(Json(serde_json::json!({
        "group": GroupResponse::from(g),
        // A group may own private repositories; do not disclose them to a
        // caller who cannot read them.
        "repos": repos
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "owner": r.owner_name,
                    "name": r.name,
                    "full_name": format!("{}/{}", r.owner_name, r.name),
                    "description": r.description,
                    "is_private": r.is_private,
                    "default_branch": r.default_branch,
                    "created_at": r.created_at.to_rfc3339(),
                })
            })
            .collect::<Vec<_>>(),
    })))
}

async fn create_group_repo(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(group): Path<String>,
    Json(req): Json<CreateGroupRepoRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let g = state
        .db
        .get_group_by_name(&group)
        .await?
        .ok_or_else(|| ApiError::NotFound("group not found".into()))?;

    // Only the group owner (or members) can create repos. MVP: owner.
    if g.owner_id != user.id {
        return Err(ApiError::Forbidden("only the group owner can create repositories".into()));
    }

    let name = req.name.trim().to_string();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
        return Err(ApiError::BadRequest(
            "repository name may only contain letters, numbers, '-', '_', '.'".into(),
        ));
    }
    let description = req.description.unwrap_or_default();
    let is_private = req.is_private.unwrap_or(false);
    let default_branch = req.default_branch.unwrap_or_else(|| "main".to_string());

    // Group repos use `owner_name = group name` for uniqueness.
    if state
        .db
        .get_repository_by_name(&g.name, &name)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict("repository already exists".into()));
    }

    // The group namespace is written as part of the insert, so the
    // repository is never observable without it.
    let repo = state
        .db
        .create_repository(
            g.owner_id,
            &g.name,
            &name,
            &description,
            is_private,
            &default_branch,
            Some(g.id),
        )
        .await?;

    state
        .git
        .create_repo(&g.name, &name, &default_branch)
        .await?;

    state.bus.publish(
        EventType::RepoCreated,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({"group": g.name, "repo": name}),
    );

    Ok(Json(serde_json::json!({
        "id": repo.id,
        "owner": g.name,
        "name": name,
        "full_name": format!("{}/{}", g.name, name),
        "description": repo.description,
        "is_private": repo.is_private,
        "default_branch": repo.default_branch,
        "created_at": repo.created_at.to_rfc3339(),
    })))
}
// ---------------------------------------------------------------------------
// Group member management
// ---------------------------------------------------------------------------

fn require_group_owner(state: &AppState, g: &gita_core::Group, user_id: i64) -> ApiResult<()> {
    if g.owner_id != user_id {
        return Err(ApiError::Forbidden(
            "only the group owner can manage members".into(),
        ));
    }
    let _ = state;
    Ok(())
}

async fn list_group_members(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(group): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let g = state
        .db
        .get_group_by_name(&group)
        .await?
        .ok_or_else(|| ApiError::NotFound("group not found".into()))?;
    // Membership is only visible to the group's owner and its own members —
    // an unauthenticated caller could otherwise enumerate who belongs to
    // every group in the instance.
    let is_member = state
        .db
        .get_group_member_role(g.id, user.id)
        .await?
        .is_some();
    if g.owner_id != user.id && !is_member {
        return Err(ApiError::Forbidden(
            "only the group owner or a member can list members".into(),
        ));
    }
    let members = state.db.list_group_members(g.id).await?;
    Ok(Json(serde_json::json!({ "members": members })))
}

async fn add_group_member(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(group): Path<String>,
    Json(req): Json<AddGroupMemberRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let g = state
        .db
        .get_group_by_name(&group)
        .await?
        .ok_or_else(|| ApiError::NotFound("group not found".into()))?;
    require_group_owner(&state, &g, user.id)?;

    let username = req.username.trim().to_string();
    if username.is_empty() {
        return Err(ApiError::BadRequest("username is required".into()));
    }
    let target = state
        .db
        .get_user_by_username(&username)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    let role = req.role.unwrap_or_else(|| "member".to_string());
    let role = normalize_role(&role)?;

    state
        .db
        .upsert_group_member(g.id, target.id, &role)
        .await?;

    Ok(Json(serde_json::json!({
        "group": g.name,
        "username": target.username,
        "role": role,
    })))
}

async fn remove_group_member(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((group, username)): Path<(String, String)>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let g = state
        .db
        .get_group_by_name(&group)
        .await?
        .ok_or_else(|| ApiError::NotFound("group not found".into()))?;
    require_group_owner(&state, &g, user.id)?;

    let target = state
        .db
        .get_user_by_username(&username)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    if !state
        .db
        .remove_group_member(g.id, target.id)
        .await?
    {
        return Err(ApiError::NotFound("user is not a member of this group".into()));
    }
    Ok(Json(crate::error::MessageResponse {
        message: "member removed".to_string(),
    }))
}

async fn update_group_member(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((group, username)): Path<(String, String)>,
    Json(req): Json<UpdateGroupMemberRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let g = state
        .db
        .get_group_by_name(&group)
        .await?
        .ok_or_else(|| ApiError::NotFound("group not found".into()))?;
    require_group_owner(&state, &g, user.id)?;

    let target = state
        .db
        .get_user_by_username(&username)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    let role = normalize_role(&req.role)?;

    state
        .db
        .upsert_group_member(g.id, target.id, &role)
        .await?;

    Ok(Json(serde_json::json!({
        "group": g.name,
        "username": target.username,
        "role": role,
    })))
}

/// Restrict roles to a small known set.
///
/// `owner` is reserved for the group creator. The group's `owner_id` is set
/// when the group is created and never changes, and `create_group` records
/// the creator in `group_members` with role `owner` — so an `owner` row does
/// exist in the database. What the API must not do is hand that role to
/// *anyone else*: a second member with role `owner` would look like an
/// owner while `require_group_owner` still only honours `groups.owner_id`.
fn normalize_role(role: &str) -> ApiResult<String> {
    match role {
        "maintainer" | "developer" | "member" | "guest" => Ok(role.to_string()),
        _ => Err(ApiError::BadRequest(format!(
            "invalid role '{role}' (allowed: maintainer, developer, member, guest)"
        ))),
    }
}
