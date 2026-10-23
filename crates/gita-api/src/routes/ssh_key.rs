//! SSH key management: register, list, delete a public key per user.

use crate::error::{ApiError, ApiResult};
use crate::extract::AuthUserExtractor;
use crate::state::AppState;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct AddSshKeyRequest {
    pub title: Option<String>,
    pub public_key: String,
}

#[derive(Debug, Serialize)]
pub struct SshKeyResponse {
    pub id: i64,
    pub title: String,
    pub public_key: String,
    pub created_at: String,
}

impl From<gita_db::SshKeyRow> for SshKeyResponse {
    fn from(k: gita_db::SshKeyRow) -> Self {
        Self {
            id: k.id,
            title: k.title,
            public_key: k.public_key,
            created_at: k.created_at,
        }
    }
}

pub fn ssh_key_routes() -> Router<AppState> {
    Router::new()
        .route("/api/auth/keys", get(list_keys).post(add_key))
        .route("/api/auth/keys/{id}", axum::routing::delete(delete_key))
}

async fn add_key(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Json(req): Json<AddSshKeyRequest>,
) -> ApiResult<Json<SshKeyResponse>> {
    let pk = req.public_key.trim().to_string();
    // Light sanity check: must look like an OpenSSH public key line.
    let mut parts = pk.split_whitespace();
    let alg = parts.next().unwrap_or("");
    if !matches!(
        alg,
        "ssh-rsa" | "ssh-ed25519" | "ecdsa-sha2-nistp256" | "ssh-dss"
    ) {
        return Err(ApiError::BadRequest(
            "unsupported key algorithm (expected ssh-rsa / ssh-ed25519 / ecdsa...)".into(),
        ));
    }
    if parts.next().is_none() {
        return Err(ApiError::BadRequest(
            "public key missing base64 blob".into(),
        ));
    }
    if !gita_db::ssh_key_is_valid(&pk) {
        return Err(ApiError::BadRequest(
            "public key is not a valid base64 key blob".into(),
        ));
    }
    let title = req.title.unwrap_or_default();
    let key = state.db.add_ssh_key(user.id, &title, &pk).await?;
    Ok(Json(SshKeyResponse::from(key)))
}

async fn list_keys(
    State(state): State<AppState>,
    user: AuthUserExtractor,
) -> ApiResult<Json<serde_json::Value>> {
    let keys = state.db.list_ssh_keys(user.id).await?;
    Ok(Json(serde_json::json!({
        "keys": keys.into_iter().map(SshKeyResponse::from).collect::<Vec<_>>(),
    })))
}

async fn delete_key(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let deleted = state.db.delete_ssh_key(id, user.id).await?;
    if !deleted {
        return Err(ApiError::NotFound("key not found".into()));
    }
    Ok(Json(crate::error::MessageResponse {
        message: "key deleted".to_string(),
    }))
}
