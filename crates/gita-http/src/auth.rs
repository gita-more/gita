//! Authorization for git-over-HTTP requests.

use axum::body::Body;
use axum::http::{header, HeaderMap, Response, StatusCode};
use axum::response::IntoResponse;
use base64::Engine as _;
use gita_core::access::Access;
use gita_core::User;

use crate::parse::{is_read_request, is_write_request};
use crate::GitHttpState;

/// Authorize a git-over-HTTP request.
///
/// The rule is the shared one in `gita_core::access`: a push needs
/// [`Access::Push`], everything else needs [`Access::Read`]. Only the
/// *report* of a refusal is specific to this transport — git clients
/// need a `WWW-Authenticate` challenge to be prompted for credentials,
/// so a private repository answers 401 rather than 404.
pub(crate) async fn authorize_git_request(
    state: &GitHttpState,
    owner: &str,
    repo_name: &str,
    path_info: &str,
    query_string: &Option<String>,
    headers: &HeaderMap,
) -> Result<(), Response<Body>> {
    let is_write = is_write_request(path_info, query_string);
    // Everything that is not a write is a read of the addressed
    // repository — including the "dumb" file endpoints. See
    // `is_read_request` for why this cannot be a substring match.
    let is_read = is_read_request(path_info, query_string);

    // Fetch the repository record.
    let repo = state
        .db
        .get_repository_by_name(owner, repo_name)
        .await
        .map_err(|e| {
            tracing::error!("db error: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            )
                .into_response()
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("repository not found: {owner}/{repo_name}"),
            )
                .into_response()
        })?;

    // Resolve the authenticated user from the Authorization header.
    // Git clients send `Authorization: Basic base64(username:token)` when
    // the remote URL embeds credentials, while the REST API uses Bearer.
    // Accept both: for Basic, the password component is the token (PAT or JWT).
    let authed = resolve_authed_user(state, headers).await;
    let user_id = authed.as_ref().map(|u| u.id);

    // Writes (push) need push rights; everything else needs read rights.
    let action = if is_write {
        Access::Push
    } else if is_read {
        Access::Read
    } else {
        // Unreachable in practice: `is_read` is the negation of
        // `is_write`. Treat it as a read rather than a hole.
        Access::Read
    };

    let allowed = state
        .db
        .can_access(&repo, user_id, action)
        .await
        .map_err(|e| {
            tracing::error!("db error: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            )
                .into_response()
        })?;

    if !allowed {
        if is_write {
            // Return a Basic auth challenge so git clients know to send
            // credentials (from the remote URL or credential helper).
            return Err(unauthorized_with_challenge(
                "authentication required to push (repository owner only)",
            ));
        }
        return Err(unauthorized_with_challenge("this repository is private"));
    }

    Ok(())
}

/// Build a `401 Unauthorized` response with a `WWW-Authenticate: Basic`
/// challenge header, so git clients retry with credentials.
fn unauthorized_with_challenge(message: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::WWW_AUTHENTICATE, "Basic realm=\"gita\"")
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(message.to_string()))
        .unwrap()
}

/// Resolve the authenticated user from the request's Authorization header.
///
/// Supports both `Authorization: Bearer <token>` (REST API style) and
/// `Authorization: Basic base64(username:token)` (git smart HTTP style).
/// For Basic auth, the password component is treated as the access token
/// (PAT or JWT), which mirrors how the server issues tokens to clients.
///
/// The username in a Basic credential is not decoration: it must name
/// the user the token belongs to. Ignoring it would let anyone holding
/// a token present themselves as its owner under any username, and it
/// would make a wrong username look like it had worked.
pub(crate) async fn resolve_authed_user(state: &GitHttpState, headers: &HeaderMap) -> Option<User> {
    let auth_header = headers.get(header::AUTHORIZATION)?.to_str().ok()?;

    // Bearer token.
    if let Some(token) = auth_header.strip_prefix("Bearer ") {
        return state.auth.resolve_bearer(&state.db, token).await.ok();
    }

    // Basic base64(username:password) — the password is the token.
    if let Some(encoded) = auth_header.strip_prefix("Basic ") {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .ok()?;
        let text = String::from_utf8(decoded).ok()?;
        let (username, password) = text.split_once(':')?;
        let user = state.auth.resolve_bearer(&state.db, password).await.ok()?;
        // The token must belong to the named user.
        if !user.username.eq_ignore_ascii_case(username) {
            return None;
        }
        return Some(user);
    }

    None
}
