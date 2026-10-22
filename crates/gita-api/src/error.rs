//! REST API for Gita.
//!
//! This crate exposes the `/api/*` HTTP endpoints, covering:
//!
//! - authentication (register, login, PAT tokens)
//! - repositories (CRUD, browse tree, read files, commits)
//! - pull requests (create, list, get, merge, close)
//! - issues (create, list, get, close)
//! - event stream (AI-readable)
//!
//! The API follows a GitHub-like style and is documented via utoipa.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use std::fmt;

/// API error type.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("rate limited: {0}")]
    RateLimited(String),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl ApiError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::RateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Internal(_) | ApiError::Database(_) | ApiError::Io(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // The message of an internal failure is not shown to the client.
        //
        // `ApiError::Internal` and `ApiError::Database` carry the
        // underlying error verbatim, and those errors name absolute
        // filesystem paths (`open bare repo /srv/gita/data/repos/...`),
        // table and column names, and constraint names. Handing them to
        // an arbitrary caller discloses the server's directory layout and
        // database schema. The detail goes to the log instead, where it
        // belongs.
        let message = match &self {
            ApiError::Internal(_) | ApiError::Database(_) | ApiError::Io(_) => {
                tracing::error!("internal error: {}", self);
                "internal error".to_string()
            }
            _ => self.to_string(),
        };
        let body = serde_json::json!({
            "error": {
                "message": message,
                "status": self.status_code().as_u16(),
            }
        });
        (self.status_code(), axum::Json(body)).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::Internal(e.to_string())
    }
}

/// Wrap a `Result` into an axum response.
pub type ApiResult<T> = Result<T, ApiError>;

/// The current authenticated user, extracted from JWT or Bearer token.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: i64,
    pub username: String,
}

impl fmt::Display for AuthUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.username)
    }
}

/// A simple response wrapper for messages.
#[derive(Debug, Clone, Serialize)]
pub struct MessageResponse {
    pub message: String,
}

pub fn message(msg: &str) -> axum::Json<MessageResponse> {
    axum::Json(MessageResponse {
        message: msg.to_string(),
    })
}