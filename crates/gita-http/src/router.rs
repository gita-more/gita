//! The `/git/*` router and its request handler.

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{body::to_bytes, Router};
use tracing::debug;

use crate::auth::{authorize_git_request, resolve_authed_user};
use crate::parse::parse_repo_from_path;
use crate::GitHttpState;

/// Build the router for `/git/*` endpoints.
pub fn router(state: GitHttpState) -> Router {
    Router::new()
        .route(
            "/git/{*path}",
            get(handle_git_request).post(handle_git_request),
        )
        .with_state(state)
}

async fn handle_git_request(
    axum::extract::State(state): axum::extract::State<GitHttpState>,
    axum::extract::Path(path): axum::extract::Path<String>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response<Body> {
    let method = request.method().to_string();
    let query_string = request.uri().query().map(|s| s.to_string());

    // Build the PATH_INFO: /git/owner/repo.git/info/refs
    // The path param includes everything after /git/
    let path_info = format!("/{}", path.trim_start_matches('/'));

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/x-git-upload-pack-request")
        .to_string();

    // Read the request body.
    let body = match to_bytes(request.into_body(), 512 * 1024 * 1024).await {
        Ok(b) => b.to_vec(),
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("failed to read body: {e}")).into_response();
        }
    };

    debug!(
        method = %method,
        path_info = %path_info,
        content_type = %content_type,
        body_len = body.len(),
        "git http request"
    );

    // Extract owner/repo from the path to emit events and check existence.
    let (owner, repo_name) = parse_repo_from_path(&path_info);

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

    // Authorize the request: writes require the repository owner;
    // reads of private repositories require the owner, while public
    // repositories are readable anonymously.
    if let Some(owner) = &owner {
        if let Some(repo_name) = &repo_name {
            if let Err(resp) = authorize_git_request(
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
    }

    // Parse ref updates from receive-pack requests BEFORE running the CGI.
    // The request body is the protocol's authoritative source for what a
    // push claims to update — the CGI's report-status body is streamed in
    // chunks and hard to recover synchronously. We keep the parsed updates
    // in scope for the post-CGI publish below.
    let received_updates = if path_info.contains("git-receive-pack") {
        gita_push::parse_receive_pack_commands(&body)
    } else {
        Vec::new()
    };

    let (status, cgi_headers, body) = match state.git.run_http_backend(
        &method,
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

    // Publish events on successful push.
    if path_info.contains("git-receive-pack") && status < 300 {
        if let (Some(owner), Some(repo_name)) = (&owner, &repo_name) {
            // Resolve who pushed — this fixes the long-standing
            // actor_id: None on push events and lets CI enforce policies
            // like "don't run on pushes from bot users".
            let pusher = resolve_authed_user(&state, &headers).await;
            let pusher_id = pusher.as_ref().map(|u| u.id);
            // `git http-backend` answers a *rejected* push with HTTP 200 as
            // well — the refusal is reported inside the response body
            // (`report-status`), not in the status code. The ref updates
            // parsed from the request body are therefore only what the
            // client *claimed* it was pushing, not what actually happened.
            //
            // Verify each one against the repository: a ref update counts
            // as applied only if the ref now points at the new sha.
            // Otherwise a rejected push would publish a `push` event with
            // a sha that was never written, and would trigger CI for a
            // commit that does not exist.
            let applied_updates: Vec<gita_push::RefUpdate> = received_updates
                .iter()
                .filter(|upd| gita_push::ref_update_applied(&state.git, owner, repo_name, upd))
                .cloned()
                .collect();
            gita_push::publish_push(
                &state.db,
                &state.bus,
                &state.git,
                owner,
                repo_name,
                &applied_updates,
                pusher_id,
            )
            .await;
        }
    }

    // Build the axum response from CGI output.
    let mut builder =
        Response::builder().status(StatusCode::from_u16(status).unwrap_or(StatusCode::OK));
    for (key, value) in cgi_headers {
        if key.eq_ignore_ascii_case("Content-Type") {
            if let Ok(v) = value.parse::<axum::http::HeaderValue>() {
                builder = builder.header(header::CONTENT_TYPE, v);
            }
        } else if key.eq_ignore_ascii_case("Content-Length") {
            // We'll set the body length automatically.
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
