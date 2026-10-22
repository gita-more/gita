//! Extractors for authenticated users.

use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use gita_core::User;

/// Extractor resolving the authenticated user from the
/// `Authorization: Bearer <jwt-or-pat>` header.
#[derive(Debug, Clone)]
pub struct AuthUserExtractor {
    pub id: i64,
    pub username: String,
}
impl FromRequestParts<AppState> for AuthUserExtractor {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ApiError::Unauthorized("missing Authorization header".into()))?;

        let token = header
            .strip_prefix("Bearer ")
            .ok_or_else(|| ApiError::Unauthorized("expected Bearer token".into()))?;

        let user: User = state.auth.resolve_bearer(&state.db, token).await?;
        Ok(AuthUserExtractor {
            id: user.id,
            username: user.username,
        })
    }
}

/// Extractor that yields an optional authenticated user (anonymous allowed).
#[derive(Debug, Clone)]
pub struct OptionalAuthUser(pub Option<AuthUserExtractor>);

impl FromRequestParts<AppState> for OptionalAuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        let Some(header) = header else {
            return Ok(OptionalAuthUser(None));
        };
        let Some(token) = header.strip_prefix("Bearer ") else {
            return Ok(OptionalAuthUser(None));
        };
        match state.auth.resolve_bearer(&state.db, token).await {
            Ok(user) => Ok(OptionalAuthUser(Some(AuthUserExtractor {
                id: user.id,
                username: user.username,
            }))),
            Err(e) => Err(e),
        }
    }
}

/// Extractor yielding the peer address of the connection, when there is
/// one.
///
/// The server is served with
/// `into_make_service_with_connect_info::<SocketAddr>()`, so every request
/// that arrives over a socket carries the real remote address. Tests and
/// in-process callers drive the router directly and have none — this
/// extractor reports that as `None` rather than rejecting the request.
///
/// It exists because axum has no `Option<ConnectInfo<_>>` handler
/// extractor: `ConnectInfo` rejects when the extension is missing, and
/// `Option<T>` only works for extractors that implement
/// `OptionalFromRequestParts`, which `ConnectInfo` does not.
#[derive(Debug, Clone)]
pub struct OptionalConnectInfo(pub Option<axum::extract::ConnectInfo<std::net::SocketAddr>>);

impl FromRequestParts<AppState> for OptionalConnectInfo {
    // Never fails: a missing peer address is a normal condition, not an error.
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(OptionalConnectInfo(
            parts
                .extensions
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .copied(),
        ))
    }
}
