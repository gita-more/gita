//! Avatar upload / serving.
//!
//! A user's avatar is a small image stored under `avatar_dir/{user_id}`.
//! We keep the original bytes (content-type sniffed at upload time) but
//! always serve it back as `image/*`; the frontend uses
//! `/api/users/{username}/avatar` in an `<img>` tag.

use crate::error::{ApiError, ApiResult};
use crate::extract::AuthUserExtractor;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use std::path::PathBuf;

pub fn avatar_routes() -> Router<AppState> {
    Router::new()
        .route("/api/users/me/avatar", axum::routing::post(upload_avatar))
        .route("/api/users/{username}/avatar", get(get_avatar))
}

async fn upload_avatar(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    mut multipart: Multipart,
) -> ApiResult<Json<serde_json::Value>> {
    let mut bytes: Option<Vec<u8>> = None;
    let mut content_type: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart error: {e}")))?
    {
        if field.name() == Some("file") {
            content_type = field.content_type().map(|s| s.to_string());
            bytes = Some(
                field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("read file error: {e}")))?
                    .to_vec(),
            );
        }
    }

    let Some(data) = bytes else {
        return Err(ApiError::BadRequest("missing 'file' field".into()));
    };
    let ct = content_type.unwrap_or_default();
    if !ct.starts_with("image/") {
        return Err(ApiError::BadRequest("file must be an image".into()));
    }
    if data.is_empty() || data.len() > 5 * 1024 * 1024 {
        return Err(ApiError::BadRequest(
            "image must be between 1 byte and 5 MiB".into(),
        ));
    }

    std::fs::create_dir_all(&state.avatar_dir)
        .map_err(|e| ApiError::Internal(format!("create avatar dir: {e}")))?;
    let file_path = avatar_path(&state.avatar_dir, user.id);
    std::fs::write(&file_path, &data)
        .map_err(|e| ApiError::Internal(format!("write avatar: {e}")))?;

    Ok(Json(json!({
        "avatar_url": format!("/api/users/{}/avatar", user.username),
        "content_type": ct,
        "bytes": data.len(),
    })))
}

async fn get_avatar(
    State(state): State<AppState>,
    Path(username): Path<String>,
) -> ApiResult<Response> {
    let user = state
        .db
        .get_user_by_username(&username)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    let file_path = avatar_path(&state.avatar_dir, user.id);
    if !file_path.exists() {
        return Err(ApiError::NotFound("avatar not found".into()));
    }
    let data = std::fs::read(&file_path).map_err(|e| ApiError::Io(e))?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/*")
        .header(header::CACHE_CONTROL, "public, max-age=300")
        .body(Body::from(data))
        .map_err(|e| ApiError::Internal(format!("response error: {e}")))?)
}

fn avatar_path(dir: &PathBuf, user_id: i64) -> PathBuf {
    dir.join(format!("user-{user_id}.img"))
}
