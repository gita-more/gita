//! Group routes: create, list, get group details + its repositories,
//! and create a repository inside a group.

use crate::error::{ApiError, ApiResult};
use crate::extract::{AuthUserExtractor, OptionalAuthUser};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::{Json, Router};
use gita_core::EventType;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct CreateGroupRequest {
    pub name: String,
    pub description: Option<String>,
    pub parent_path: Option<String>, // Full path of parent group, e.g. "groupa/subgroupa"
}

#[derive(Debug, Serialize)]
pub struct GroupResponse {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub owner_id: i64,
    pub created_at: String,
    pub parent_id: Option<i64>,
    pub full_path: String,
}

impl From<gita_core::Group> for GroupResponse {
    fn from(g: gita_core::Group) -> Self {
        Self {
            id: g.id,
            name: g.name,
            description: g.description,
            owner_id: g.owner_id,
            created_at: g.created_at.to_rfc3339(),
            parent_id: g.parent_id,
            full_path: g.full_path,
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
        .route(
            "/api/groups",
            axum::routing::post(create_group).get(list_groups),
        )
        .route(
            "/api/groups/{*path}",
            axum::routing::get(get_group_or_subgroups)
                .post(create_group_repo_or_subgroup)
                .patch(update_group_member_dispatch)
                .delete(delete_group_member_or_repo),
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
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(ApiError::BadRequest(
            "group name may only contain letters, numbers, '-', '_', '.'".into(),
        ));
    }
    let description = req.description.unwrap_or_default();

    let parent_id = if let Some(parent_path) = req.parent_path {
        let parent = state
            .db
            .get_group_by_full_path(&parent_path)
            .await?
            .ok_or_else(|| ApiError::NotFound("parent group not found".into()))?;
        Some(parent.id)
    } else {
        None
    };

    // Check if group already exists under the same parent
    let parent_check = if let Some(pid) = parent_id {
        format!("parent_id = {}", pid)
    } else {
        "parent_id IS NULL".to_string()
    };
    let exists = sqlx::query(&format!(
        "SELECT 1 FROM groups WHERE name = $1 AND ({} OR ({} AND $1 IS NULL))",
        parent_check, parent_check
    ))
    .bind(&name)
    .fetch_optional(state.db.pool())
    .await?
    .is_some();
    if exists {
        return Err(ApiError::Conflict("group already exists".into()));
    }

    let group = state.db.create_group(&name, &description, user.id, parent_id).await?;

    state.bus.publish(
        EventType::RepoCreated,
        None,
        Some(user.id),
        serde_json::json!({"group": group.full_path}),
    );

    Ok(Json(GroupResponse::from(group)))
}

/// Dispatch POST /api/groups/{*path} to the right handler based on path suffix.
async fn create_group_repo_or_subgroup(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(path): Path<String>,
    axum::extract::Json(body): axum::extract::Json<Value>,
) -> ApiResult<axum::response::Response> {
    if path.ends_with("/repos") {
        let group_path = path.strip_suffix("/repos").unwrap();
        let req: CreateGroupRepoRequest = serde_json::from_value(body)
            .map_err(|e| ApiError::BadRequest(format!("invalid request body: {e}")))?;
        let res = create_group_repo(State(state.clone()), user, Path(group_path.to_string()), Json(req)).await?;
        Ok(res.into_response())
    } else if path.ends_with("/subgroups") {
        let group_path = path.strip_suffix("/subgroups").unwrap();
        let req: CreateGroupRequest = serde_json::from_value(body)
            .map_err(|e| ApiError::BadRequest(format!("invalid request body: {e}")))?;
        let res = create_subgroup(State(state.clone()), user, Path(group_path.to_string()), Json(req)).await?;
        Ok(res.into_response())
    } else if path.ends_with("/members") {
        let group_path = path.strip_suffix("/members").unwrap();
        let req: AddGroupMemberRequest = serde_json::from_value(body)
            .map_err(|e| ApiError::BadRequest(format!("invalid request body: {e}")))?;
        let res = add_group_member(State(state.clone()), user, Path(group_path.to_string()), Json(req)).await?;
        Ok(res.into_response())
    } else {
        Err(ApiError::NotFound("endpoint not found".into()))
    }
}

/// Dispatch PATCH /api/groups/{*path} to the right handler.
async fn update_group_member_dispatch(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(path): Path<String>,
    axum::extract::Json(body): axum::extract::Json<Value>,
) -> ApiResult<axum::response::Response> {
    if path.contains("/members/") {
        let parts: Vec<&str> = path.split("/members/").collect();
        if parts.len() == 2 {
            let req: UpdateGroupMemberRequest = serde_json::from_value(body)
                .map_err(|e| ApiError::BadRequest(format!("invalid request body: {e}")))?;
            let res = update_group_member_inner(State(state.clone()), user, Path((parts[0].to_string(), parts[1].to_string())), Json(req)).await?;
            Ok(res.into_response())
        } else {
            Err(ApiError::NotFound("endpoint not found".into()))
        }
    } else {
        Err(ApiError::NotFound("endpoint not found".into()))
    }
}

/// Dispatch DELETE /api/groups/{*path} to the right handler.
async fn delete_group_member_or_repo(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(path): Path<String>,
) -> ApiResult<axum::response::Response> {
    if path.contains("/members/") {
        let parts: Vec<&str> = path.split("/members/").collect();
        if parts.len() == 2 {
            let res = remove_group_member(State(state.clone()), user, Path((parts[0].to_string(), parts[1].to_string()))).await?;
            Ok(res.into_response())
        } else {
            Err(ApiError::NotFound("endpoint not found".into()))
        }
    } else {
        Err(ApiError::NotFound("endpoint not found".into()))
    }
}

async fn list_groups(
    State(state): State<AppState>,
    user: AuthUserExtractor,
) -> ApiResult<Json<Vec<GroupResponse>>> {
    let groups = state.db.list_groups_for_user(user.id).await?;
    Ok(Json(groups.into_iter().map(GroupResponse::from).collect()))
}

async fn get_group_or_subgroups(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path(path): Path<String>,
) -> ApiResult<axum::response::Response> {
    if path.ends_with("/subgroups") {
        let group_path = path.strip_suffix("/subgroups").unwrap();
        let auth_user = user.0.ok_or_else(|| ApiError::Unauthorized("authentication required".into()))?;
        let res = list_subgroups(State(state.clone()), auth_user, Path(group_path.to_string())).await?;
        Ok(res.into_response())
    } else if path.ends_with("/members") {
        let group_path = path.strip_suffix("/members").unwrap();
        let auth_user = user.0.ok_or_else(|| ApiError::Unauthorized("authentication required".into()))?;
        let res = list_group_members(State(state.clone()), auth_user, Path(group_path.to_string())).await?;
        Ok(res.into_response())
    } else if path.ends_with("/repos") {
        let group_path = path.strip_suffix("/repos").unwrap();
        let auth_user = user.0.ok_or_else(|| ApiError::Unauthorized("authentication required".into()))?;
        let res = list_group_repos(State(state.clone()), auth_user, Path(group_path.to_string())).await?;
        Ok(res.into_response())
    } else {
        let res = get_group(State(state), user, Path(path)).await?;
        Ok(res.into_response())
    }
}
    async fn get_group(
    State(state): State<AppState>,
    user: OptionalAuthUser,
    Path(path): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let g = state
        .db
        .get_group_by_full_path(&path)
        .await?
        .ok_or_else(|| ApiError::NotFound("group not found".into()))?;
    let repos = state
        .db
        .filter_readable(
            state.db.list_group_repositories(g.id).await?,
            user.0.as_ref().map(|u| u.id),
        )
        .await?;
    let subgroups = state.db.list_subgroups(g.id).await?;
    Ok(Json(serde_json::json!({
        "group": GroupResponse::from(g),
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
        "subgroups": subgroups
            .into_iter()
            .map(GroupResponse::from)
            .collect::<Vec<_>>(),
    })))
}

async fn list_subgroups(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(path): Path<String>,
) -> ApiResult<Json<Vec<GroupResponse>>> {
    let g = state
        .db
        .get_group_by_full_path(&path)
        .await?
        .ok_or_else(|| ApiError::NotFound("group not found".into()))?;

    // Check membership
    let is_member = state
        .db
        .get_group_member_role(g.id, user.id)
        .await?
        .is_some();
    if g.owner_id != user.id && !is_member {
        return Err(ApiError::Forbidden(
            "only the group owner or a member can list subgroups".into(),
        ));
    }

    let subgroups = state.db.list_subgroups(g.id).await?;
    Ok(Json(subgroups.into_iter().map(GroupResponse::from).collect()))
}

/// GET /api/groups/{path}/repos — list a group's repositories (bare array).
///
/// Like the member list, repositories of a group are only enumerable by
/// the group's owner and its members; anonymous callers use the group
/// detail endpoint, which filters to readable repositories.
async fn list_group_repos(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(path): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let g = state
        .db
        .get_group_by_full_path(&path)
        .await?
        .ok_or_else(|| ApiError::NotFound("group not found".into()))?;
    let is_member = state
        .db
        .get_group_member_role(g.id, user.id)
        .await?
        .is_some();
    if g.owner_id != user.id && !is_member {
        return Err(ApiError::Forbidden(
            "only the group owner or a member can list repositories".into(),
        ));
    }
    let repos = state.db.list_group_repositories(g.id).await?;
    let readable = state
        .db
        .filter_readable(repos, Some(user.id))
        .await?;
    Ok(Json(serde_json::json!(readable
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
        .collect::<Vec<_>>())))
}

async fn create_subgroup(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(path): Path<String>,
    Json(req): Json<CreateGroupRequest>,
) -> ApiResult<Json<GroupResponse>> {
    let g = state
        .db
        .get_group_by_full_path(&path)
        .await?
        .ok_or_else(|| ApiError::NotFound("group not found".into()))?;

    // Only group owner can create subgroups
    if g.owner_id != user.id {
        return Err(ApiError::Forbidden(
            "only the group owner can create subgroups".into(),
        ));
    }

    let name = req.name.trim().to_string();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(ApiError::BadRequest(
            "group name may only contain letters, numbers, '-', '_', '.'".into(),
        ));
    }
    let description = req.description.unwrap_or_default();

    let group = state
        .db
        .create_group(&name, &description, user.id, Some(g.id))
        .await?;

    state.bus.publish(
        EventType::RepoCreated,
        None,
        Some(user.id),
        serde_json::json!({"group": group.full_path}),
    );

    Ok(Json(GroupResponse::from(group)))
}

async fn create_group_repo(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(path): Path<String>,
    Json(req): Json<CreateGroupRepoRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let g = state
        .db
        .get_group_by_full_path(&path)
        .await?
        .ok_or_else(|| ApiError::NotFound("group not found".into()))?;

    // Only the group owner (or members) can create repos. MVP: owner.
    if g.owner_id != user.id {
        return Err(ApiError::Forbidden(
            "only the group owner can create repositories".into(),
        ));
    }

    let name = req.name.trim().to_string();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(ApiError::BadRequest(
            "repository name may only contain letters, numbers, '-', '_', '.'".into(),
        ));
    }
    let description = req.description.unwrap_or_default();
    let is_private = req.is_private.unwrap_or(false);
    let default_branch = req.default_branch.unwrap_or_else(|| "main".to_string());

    // Group repos use `owner_name = group full_path` for uniqueness.
    if state
        .db
        .get_repository_by_name(&g.full_path, &name)
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
            &g.full_path,
            &name,
            &description,
            is_private,
            &default_branch,
            Some(g.id),
        )
        .await?;

    state
        .git
        .create_repo(&g.full_path, &name, &default_branch)
        .await?;

    state.bus.publish(
        EventType::RepoCreated,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({"group": g.full_path, "repo": name}),
    );

    Ok(Json(serde_json::json!({
        "id": repo.id,
        "owner": g.full_path,
        "name": name,
        "full_name": format!("{}/{}", g.full_path, name),
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
    Path(path): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let g = state
        .db
        .get_group_by_full_path(&path)
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
    Path(path): Path<String>,
    Json(req): Json<AddGroupMemberRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let g = state
        .db
        .get_group_by_full_path(&path)
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

    state.db.upsert_group_member(g.id, target.id, &role).await?;

    Ok(Json(serde_json::json!({
        "group": g.full_path,
        "username": target.username,
        "role": role,
    })))
}

async fn remove_group_member(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((path, username)): Path<(String, String)>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let g = state
        .db
        .get_group_by_full_path(&path)
        .await?
        .ok_or_else(|| ApiError::NotFound("group not found".into()))?;
    require_group_owner(&state, &g, user.id)?;

    let target = state
        .db
        .get_user_by_username(&username)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    if !state.db.remove_group_member(g.id, target.id).await? {
        return Err(ApiError::NotFound(
            "user is not a member of this group".into(),
        ));
    }
    Ok(Json(crate::error::MessageResponse {
        message: "member removed".to_string(),
    }))
}

async fn update_group_member_inner(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((path, username)): Path<(String, String)>,
    Json(req): Json<UpdateGroupMemberRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let g = state
        .db
        .get_group_by_full_path(&path)
        .await?
        .ok_or_else(|| ApiError::NotFound("group not found".into()))?;
    require_group_owner(&state, &g, user.id)?;

    let target = state
        .db
        .get_user_by_username(&username)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    let role = normalize_role(&req.role)?;

    state.db.upsert_group_member(g.id, target.id, &role).await?;

    Ok(Json(serde_json::json!({
        "group": g.full_path,
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
