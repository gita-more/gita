//! The Git smart-HTTP middleware.
//!
//! This module provides an axum middleware that intercepts git smart-HTTP
//! requests (identified by the `.git/` path pattern) and handles them
//! directly. This avoids catch-all route conflicts with the Pages router
//! and SPA fallback, which also use catch-all patterns at the root path.

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::body::to_bytes;
use tracing::debug;

use crate::auth::resolve_authed_user;
use crate::GitHttpState;

/// Git smart-HTTP middleware handler.
///
/// Git smart-HTTP requests have the pattern `owner/repo.git/...` (the
/// `.git` suffix is the discriminator). This middleware runs before
/// the Pages router and SPA fallback, intercepting only those paths
/// and handling them directly. Everything else falls through to the
/// main router.
pub async fn git_middleware(
    state: GitHttpState,
    req: Request<Body>,
    next: Next,
) -> Response<Body>
where
    GitHttpState: Clone + Send + Sync + 'static,
    Body: Send + 'static,
{
    // Extract path before consuming req.
    let path = req.uri().path().to_string();
    // Intercept only paths that look like git smart-HTTP:
    // they contain ".git/" (e.g. "owner/repo.git/info/refs") or end with ".git"
    if !path.contains(".git/") && !path.ends_with(".git") {
        return next.run(req).await;
    }

    // Extract everything we need before consuming req.
    let method = req.method().clone();
    let query_string = req.uri().query().map(|s| s.to_string());
    let headers = req.headers().clone();
    // The path is like "/owner/repo.git/info/refs" - strip leading "/" then add it back
    // git http-backend expects PATH_INFO to start with "/"
    let path_info = format!("/{}", req.uri().path().trim_start_matches('/'));
    let body = match to_bytes(req.into_body(), 512 * 1024 * 1024).await {
        Ok(b) => b.to_vec(),
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("failed to read body: {e}")).into_response();
        }
    };

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/x-git-upload-pack-request")
        .to_string();

    debug!(
        method = %method,
        path_info = %path_info,
        content_type = %content_type,
        body_len = body.len(),
        "git http request (middleware)"
    );

    let (owner, repo_name) = crate::parse::parse_repo_from_path(&path_info);

    // For receive-pack (push), we need to know the repository exists.
    if path_info.contains("git-receive-pack") {
        if let (Some(owner), Some(repo_name)) = (&owner, &repo_name) {
            if !state.git.repo_exists(owner, repo_name) {
                return (
                    StatusCode::NOT_FOUND,
                    format!("repository not found: {owner}/{repo_name}"),
                )
                    .into_response();
            }
        }
    }

    // Authorize the request: writes require push rights; reads of private
    // repositories require the owner. Public repositories are readable
    // anonymously. A private repository answers 401 (the git client needs
    // the challenge), matching the original `/git/*` handler behavior.
    if let (Some(owner), Some(repo_name)) = (&owner, &repo_name) {
        if let Err(resp) = crate::auth::authorize_git_request(
            &state,
            owner,
            repo_name,
            &path_info,
            &query_string,
            &headers,
        )
        .await
        {
            return resp;
        }
    }

    let received_updates = if path_info.contains("git-receive-pack") {
        gita_push::parse_receive_pack_commands(&body)
    } else {
        Vec::new()
    };

    let (status, cgi_headers, body) = match state.git.run_http_backend(
        method.as_str(),
        &path_info,
        query_string.as_deref(),
        &content_type,
        &body,
    ) {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("git http-backend error: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            )
                .into_response();
        }
    };

    if path_info.contains("git-receive-pack") && status < 300 {
        let pusher = resolve_authed_user(&state, &headers).await;
        let pusher_id = pusher.as_ref().map(|u| u.id);
        if let (Some(owner), Some(repo)) = (&owner, &repo_name) {
            let applied_updates: Vec<gita_push::RefUpdate> = received_updates
                .iter()
                .filter(|upd| gita_push::ref_update_applied(&state.git, owner, repo, upd))
                .cloned()
                .collect();
            gita_push::publish_push(
                &state.db,
                &state.bus,
                &state.git,
                owner,
                repo,
                &applied_updates,
                pusher_id,
            )
            .await;
        }
    }

    let mut builder =
        Response::builder().status(StatusCode::from_u16(status).unwrap_or(StatusCode::OK));
    for (key, value) in cgi_headers {
        if key.eq_ignore_ascii_case("Content-Type") {
            if let Ok(v) = value.parse::<axum::http::HeaderValue>() {
                builder = builder.header(header::CONTENT_TYPE, v);
            }
        } else if key.eq_ignore_ascii_case("Content-Length") {
            continue;
        } else if key.eq_ignore_ascii_case("Expires") || key.eq_ignore_ascii_case("Pragma") {
            continue;
        } else if key.eq_ignore_ascii_case("Cache-Control") {
            if let Ok(v) = value.parse::<axum::http::HeaderValue>() {
                builder = builder.header(header::CACHE_CONTROL, v);
            }
        } else {
            if let Ok(v) = value.parse::<axum::http::HeaderValue>() {
                if let Ok(name) = header::HeaderName::from_bytes(key.as_bytes()) {
                    builder = builder.header(name, v);
                }
            }
        }
    }

    builder.body(Body::from(body)).unwrap_or_else(|_| {
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::empty())
            .unwrap()
    })
}
