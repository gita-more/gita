//! Authentication routes: register, login, personal access tokens.

use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::extract::AuthUserExtractor;
use crate::state::AppState;
use axum::extract::State;
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
}

impl From<gita_core::User> for PublicUser {
    fn from(u: gita_core::User) -> Self {
        Self {
            id: u.id,
            username: u.username,
            email: u.email,
        }
    }
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
        .route("/api/auth/me", axum::routing::get(me))
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

    let hash = Auth::hash_password(&req.password)?;
    let user = state
        .db
        .create_user(&username, &email, &hash)
        .await?;
    let token = state.auth.sign_jwt(&user)?;

    state.bus.publish(
        EventType::RepoCreated, // placeholder: no dedicated signup event; use comment_added? Keep simple.
        None,
        Some(user.id),
        serde_json::json!({"username": username}),
    );

    Ok(Json(AuthResponse {
        token,
        user: PublicUser::from(user),
    }))
}

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Json<AuthResponse>> {
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

async fn me(State(state): State<AppState>, user: AuthUserExtractor) -> ApiResult<Json<PublicUser>> {
    let db_user = state
        .db
        .get_user_by_id(user.id)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("user not found".into()))?;
    Ok(Json(PublicUser::from(db_user)))
}