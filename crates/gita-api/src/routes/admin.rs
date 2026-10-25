//! Admin-only user management routes.
//!
//! These endpoints let a system administrator (a user with `is_admin = true`)
//! list, create, update and delete accounts from the settings page or the
//! CLI. Every handler requires the caller's own `is_admin` flag; a
//! non-admin is rejected with a 403 before touching any data.

use crate::error::{ApiError, ApiResult};
use crate::extract::AuthUserExtractor;
use crate::routes::auth::PublicUser;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use gita_core::EventType;
use serde::Deserialize;
use serde_json::json;

/// Reject a caller who is not a system administrator.
fn require_admin(user: &AuthUserExtractor) -> ApiResult<()> {
    if !user.is_admin {
        return Err(ApiError::Forbidden("admin privileges required".into()));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    /// UI language preference ("zh" | "en"); defaults to "zh".
    #[serde(default)]
    pub locale: Option<String>,
    /// Whether the new account is an administrator. Defaults to false.
    #[serde(default)]
    pub is_admin: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct UpdateUserRequest {
    pub is_admin: Option<bool>,
    pub email: Option<String>,
    pub locale: Option<String>,
    pub password: Option<String>,
}

pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/users", get(list_users).post(create_user))
        .route(
            "/api/admin/users/{username}",
            axum::routing::patch(update_user).delete(delete_user),
        )
}

async fn list_users(
    State(state): State<AppState>,
    user: AuthUserExtractor,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    let users = state.db.list_users(1000).await?;
    Ok(Json(json!({
        "users": users.into_iter().map(PublicUser::from).collect::<Vec<_>>(),
    })))
}

async fn create_user(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Json(req): Json<CreateUserRequest>,
) -> ApiResult<Json<PublicUser>> {
    require_admin(&user)?;

    let username = req.username.trim().to_string();
    let email = req.email.trim().to_string();
    if username.is_empty() || username.len() < 2 {
        return Err(ApiError::BadRequest(
            "username must be at least 2 characters".into(),
        ));
    }
    // Same username rules as self-registration: it becomes a directory name
    // under the repositories root, so it must not be able to escape it.
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(ApiError::BadRequest(
            "username may only contain letters, numbers, '-', '_', '.'".into(),
        ));
    }
    if username == "." || username == ".." {
        return Err(ApiError::BadRequest("invalid username".into()));
    }
    if email.is_empty() || !email.contains('@') {
        return Err(ApiError::BadRequest("invalid email".into()));
    }
    if req.password.len() < 6 {
        return Err(ApiError::BadRequest(
            "password must be at least 6 characters".into(),
        ));
    }
    let locale = req.locale.as_deref().unwrap_or("zh");
    if locale != "zh" && locale != "en" {
        return Err(ApiError::BadRequest("invalid locale".into()));
    }

    // Uniqueness — same merged message as self-registration so the endpoint
    // does not leak which credential collided.
    if state.db.get_user_by_username(&username).await?.is_some()
        || state.db.get_user_by_email(&email).await?.is_some()
    {
        return Err(ApiError::Conflict(
            "username or email already in use".into(),
        ));
    }

    let hash = crate::auth::Auth::hash_password(&req.password)?;
    let created = state
        .db
        .create_user_with_role(
            &username,
            &email,
            &hash,
            locale,
            req.is_admin.unwrap_or(false),
        )
        .await?;

    // Same event self-registration publishes; the actor here is the
    // provisioning admin rather than the new user.
    state.bus.publish(
        EventType::UserRegistered,
        None,
        Some(user.id),
        serde_json::json!({"username": username}),
    );

    Ok(Json(PublicUser::from(created)))
}

async fn update_user(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(username): Path<String>,
    req: Option<Json<UpdateUserRequest>>,
) -> ApiResult<Json<PublicUser>> {
    require_admin(&user)?;
    let req = req.map(|Json(r)| r).unwrap_or_default();

    let target = state
        .db
        .get_user_by_username(&username)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".into()))?;

    let mut db_user = target.clone();

    // Change the admin flag. Refuses to demote the very last admin (the DB
    // layer enforces this too, but we surface a friendlier error here).
    if let Some(is_admin) = req.is_admin {
        if !is_admin && db_user.is_admin {
            let admins = state.db.count_admins().await?;
            if admins <= 1 {
                return Err(ApiError::BadRequest(
                    "cannot demote the last administrator: at least one admin must remain".into(),
                ));
            }
        }
        db_user = state
            .db
            .update_user_admin(target.id, is_admin)
            .await?
            .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    }

    if let Some(email) = req.email {
        let email = email.trim().to_string();
        if email.is_empty() || !email.contains('@') {
            return Err(ApiError::BadRequest("invalid email".into()));
        }
        if let Some(other) = state.db.get_user_by_email(&email).await? {
            if other.id != target.id {
                return Err(ApiError::Conflict("email already registered".into()));
            }
        }
        db_user = state
            .db
            .update_user_email(target.id, &email)
            .await?
            .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    }

    if let Some(locale) = req.locale {
        let locale = locale.trim().to_string();
        if locale != "zh" && locale != "en" {
            return Err(ApiError::BadRequest("invalid locale".into()));
        }
        db_user = state
            .db
            .update_user_locale(target.id, &locale)
            .await?
            .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    }

    if let Some(password) = req.password {
        if password.len() < 6 {
            return Err(ApiError::BadRequest(
                "password must be at least 6 characters".into(),
            ));
        }
        let hash = crate::auth::Auth::hash_password(&password)?;
        state.db.update_user_password_hash(target.id, &hash).await?;
        db_user = state
            .db
            .get_user_by_id(target.id)
            .await?
            .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    }

    Ok(Json(PublicUser::from(db_user)))
}

async fn delete_user(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(username): Path<String>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    require_admin(&user)?;

    // Refuse to delete your own account through this path: a system must
    // keep at least one administrator, and deleting yourself is almost
    // always an accident.
    if username == user.username {
        return Err(ApiError::BadRequest(
            "cannot delete your own account".into(),
        ));
    }

    let target = state
        .db
        .get_user_by_username(&username)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".into()))?;

    // Refuse to delete the last remaining administrator.
    if target.is_admin {
        let admins = state.db.count_admins().await?;
        if admins <= 1 {
            return Err(ApiError::BadRequest(
                "cannot delete the last administrator: at least one admin must remain".into(),
            ));
        }
    }

    // Delete the user's personal repositories.
    for repo in state.db.list_repositories_owned_by(target.id).await? {
        if repo.group_id.is_none() {
            state.db.delete_repository(repo.id).await?;
            state.git.delete_repo(&repo.owner_name, &repo.name)?;
        }
    }

    // Delete the groups the user owns (and every repository inside them).
    // Group members cascade; repositories do not, so they are removed first.
    for g in state.db.list_groups_owned_by(target.id).await? {
        for repo in state.db.list_group_repositories(g.id).await? {
            state.db.delete_repository(repo.id).await?;
            state.git.delete_repo(&repo.owner_name, &repo.name)?;
        }
        state.db.delete_group(g.id).await?;
    }

    state.db.delete_user(target.id).await?;
    Ok(Json(crate::error::MessageResponse {
        message: "user deleted".to_string(),
    }))
}
