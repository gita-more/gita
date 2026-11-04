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
//! The API follows a GitHub-like style. Its OpenAPI document is a
//! hand-written static JSON string in `docs.rs`, checked against the
//! routes the server actually registers.

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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    async fn response_body(res: Response) -> String {
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap_or_default();
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[tokio::test]
    async fn status_code_maps_each_variant() {
        assert_eq!(ApiError::BadRequest("x".into()).status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(
            ApiError::Unauthorized("x".into()).status_code(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            ApiError::Forbidden("x".into()).status_code(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            ApiError::NotFound("x".into()).status_code(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ApiError::Conflict("x".into()).status_code(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            ApiError::RateLimited("x".into()).status_code(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            ApiError::Internal("x".into()).status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn client_errors_are_returned_verbatim() {
        let err = ApiError::NotFound("repository not found".into());
        let res = err.into_response();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let body = response_body(res).await;
        assert!(body.contains("repository not found"));
        assert!(body.contains("\"status\":404"));
    }

    #[tokio::test]
    async fn internal_errors_do_not_leak_details() {
        // An absolute filesystem path and a constraint name must not reach
        // the client — they go to the log instead.
        let err = ApiError::Database(
            sqlx::Error::RowNotFound,
        );
        let res = err.into_response();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = response_body(res).await;
        assert_eq!(body, r#"{"error":{"message":"internal error","status":500}}"#);
    }

    #[tokio::test]
    async fn io_errors_do_not_leak_details() {
        let err = ApiError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no such file or directory",
        ));
        let res = err.into_response();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = response_body(res).await;
        assert_eq!(body, r#"{"error":{"message":"internal error","status":500}}"#);
    }

    #[tokio::test]
    async fn internal_error_do_not_leak_details() {
        let err = ApiError::Internal("open bare repo /srv/gita/data/repos/alice/secret.git".into());
        let res = err.into_response();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = response_body(res).await;
        assert!(!body.contains("/srv/gita"));
        assert_eq!(body, r#"{"error":{"message":"internal error","status":500}}"#);
    }

    #[tokio::test]
    async fn unauthorized_response_keeps_its_message() {
        let err = ApiError::Unauthorized("bad token".into());
        let res = err.into_response();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        let body = response_body(res).await;
        assert!(body.contains("bad token"));
        assert!(body.contains("\"status\":401"));
    }
}
