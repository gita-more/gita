//! Authentication routes: register, login, personal access tokens.

use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::extract::AuthUserExtractor;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use gita_core::EventType;
use rand::RngCore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    /// Optional UI language preference ("zh" | "en"); defaults to "zh".
    #[serde(default)]
    pub locale: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: PublicUser,
}

#[derive(Debug, Serialize)]
pub struct PublicUser {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub locale: String,
}

impl From<gita_core::User> for PublicUser {
    fn from(u: gita_core::User) -> Self {
        Self {
            id: u.id,
            username: u.username,
            email: u.email,
            locale: u.locale,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeRequest {
    pub locale: Option<String>,
    pub email: Option<String>,
    pub current_password: Option<String>,
    pub new_password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateTokenRequest {
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub id: i64,
    pub token: String,
    pub name: String,
}

pub fn auth_routes() -> Router<AppState> {
    Router::new()
        .route("/api/auth/register", post(register))
        .route("/api/auth/login", post(login))
        .route("/api/auth/tokens", post(create_token).get(list_tokens))
        .route("/api/auth/tokens/{id}", axum::routing::delete(delete_token))
        .route("/api/auth/me", axum::routing::get(me).patch(update_me))
}

#[allow(dead_code)]
pub fn auth_routes_except_login() -> Router<AppState> {
    Router::new()
        .route("/api/auth/register", post(register))
        .route("/api/auth/tokens", post(create_token).get(list_tokens))
        .route("/api/auth/tokens/{id}", axum::routing::delete(delete_token))
        .route("/api/auth/me", axum::routing::get(me).patch(update_me))
}

async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> ApiResult<Json<AuthResponse>> {
    let username = req.username.trim().to_string();
    let email = req.email.trim().to_string();
    if username.is_empty() || username.len() < 2 {
        return Err(ApiError::BadRequest("username must be at least 2 characters".into()));
    }
    if email.is_empty() || !email.contains('@') {
        return Err(ApiError::BadRequest("invalid email".into()));
    }
    if req.password.len() < 6 {
        return Err(ApiError::BadRequest("password must be at least 6 characters".into()));
    }

    // Check uniqueness.
    if let Some(_) = state.db.get_user_by_username(&username).await? {
        return Err(ApiError::Conflict("username already taken".into()));
    }
    if let Some(_) = state.db.get_user_by_email(&email).await? {
        return Err(ApiError::Conflict("email already registered".into()));
    }

    // Validate locale if provided (default "zh").
    let locale = req.locale.as_deref().unwrap_or("zh");
    if locale != "zh" && locale != "en" {
        return Err(ApiError::BadRequest("invalid locale".into()));
    }

    let hash = Auth::hash_password(&req.password)?;
    let user = state
        .db
        .create_user_with_locale(&username, &email, &hash, locale)
        .await?;
    let token = state.auth.sign_jwt(&user)?;

    state.bus.publish(
        EventType::UserRegistered,
        None,
        Some(user.id),
        serde_json::json!({"username": username}),
    );

    Ok(Json(AuthResponse {
        token,
        user: PublicUser::from(user),
    }))
}

pub async fn login(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Json<AuthResponse>> {
    // Per-IP rate limiting (P1). Bucket is per-IP; the X-Forwarded-For
    // header is honored when present so the limiter is correct behind a
    // trusted reverse proxy (nginx/docker-compose sets this).
    if let Some(limiter) = state.login_limiter.as_ref() {
        let key = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        if limiter.check_key(&key).is_err() {
            return Err(ApiError::RateLimited("too many login attempts; try again later".into()));
        }
    }
    let user = state
        .db
        .get_user_by_username(&req.username)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("invalid credentials".into()))?;
    if !Auth::verify_password(&req.password, &user.password_hash) {
        return Err(ApiError::Unauthorized("invalid credentials".into()));
    }
    let token = state.auth.sign_jwt(&user)?;
    Ok(Json(AuthResponse {
        token,
        user: PublicUser::from(user),
    }))
}

async fn create_token(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Json(req): Json<CreateTokenRequest>,
) -> ApiResult<Json<TokenResponse>> {
    let name = req.name.unwrap_or_else(|| "default".to_string());
    // Generate a random 32-byte token, hex-encoded.
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = hex::encode(bytes);

    let created = state
        .db
        .create_access_token(user.id, &token, &name)
        .await?;
    Ok(Json(TokenResponse {
        id: created.id,
        token,
        name: created.name,
    }))
}

async fn list_tokens(
    State(state): State<AppState>,
    user: AuthUserExtractor,
) -> ApiResult<Json<Vec<TokenResponse>>> {
    let tokens = state.db.list_tokens(user.id).await?;
    Ok(Json(
        tokens
            .into_iter()
            .map(|t| TokenResponse {
                id: t.id,
                token: t.token,
                name: t.name,
            })
            .collect(),
    ))
}

async fn delete_token(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(id): Path<i64>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let deleted = state.db.delete_token(id, user.id).await?;
    if !deleted {
        return Err(ApiError::NotFound("token not found".into()));
    }
    Ok(Json(crate::error::MessageResponse {
        message: "token deleted".to_string(),
    }))
}

async fn me(State(state): State<AppState>, user: AuthUserExtractor) -> ApiResult<Json<PublicUser>> {
    let db_user = state
        .db
        .get_user_by_id(user.id)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("user not found".into()))?;
    Ok(Json(PublicUser::from(db_user)))
}

async fn update_me(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Json(req): Json<UpdateMeRequest>,
) -> ApiResult<Json<PublicUser>> {
    let mut db_user = state
        .db
        .get_user_by_id(user.id)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("user not found".into()))?;

    // Email change.
    if let Some(email) = req.email {
        let email = email.trim().to_string();
        if email.is_empty() || !email.contains('@') {
            return Err(ApiError::BadRequest("invalid email".into()));
        }
        // Verify current password for sensitive changes.
        let current = req
            .current_password
            .as_deref()
            .ok_or_else(|| ApiError::BadRequest("current_password required to change email".into()))?;
        if !Auth::verify_password(current, &db_user.password_hash) {
            return Err(ApiError::Unauthorized("invalid credentials".into()));
        }
        // Ensure the new email is unique.
        if let Some(other) = state.db.get_user_by_email(&email).await? {
            if other.id != user.id {
                return Err(ApiError::Conflict("email already registered".into()));
            }
        }
        db_user = state
            .db
            .update_user_email(user.id, &email)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("user not found".into()))?;
    }

    // Password change.
    if let Some(new_password) = req.new_password {
        if new_password.len() < 6 {
            return Err(ApiError::BadRequest("password must be at least 6 characters".into()));
        }
        let current = req
            .current_password
            .as_deref()
            .ok_or_else(|| ApiError::BadRequest("current_password required to change password".into()))?;
        if !Auth::verify_password(current, &db_user.password_hash) {
            return Err(ApiError::Unauthorized("invalid credentials".into()));
        }
        let hash = Auth::hash_password(&new_password)?;
        state
            .db
            .update_user_password_hash(user.id, &hash)
            .await?;
        // Refresh user record so the response reflects the new hash.
        db_user = state
            .db
            .get_user_by_id(user.id)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("user not found".into()))?;
    }

    // Locale change.
    if let Some(locale) = req.locale {
        let locale = locale.trim().to_string();
        if locale != "zh" && locale != "en" {
            return Err(ApiError::BadRequest("invalid locale".into()));
        }
        db_user = state
            .db
            .update_user_locale(user.id, &locale)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("user not found".into()))?;
    }

    Ok(Json(PublicUser::from(db_user)))
}