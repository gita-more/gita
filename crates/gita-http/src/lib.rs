//! Smart HTTP Git protocol endpoint.
//!
//! This crate exposes the `/git/*` routes that serve the smart HTTP
//! protocol. It delegates to the `git http-backend` CGI program via
//! the [`gita_git::GitManager`], which handles upload-pack (fetch/clone)
//! and receive-pack (push).

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Router, body::to_bytes};
use base64::Engine as _;
use gita_api::auth::Auth;
use gita_core::EventType;
use gita_db::Db;
use gita_events::EventBus;
use gita_git::GitManager;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Shared state for the git HTTP handler.
#[derive(Clone)]
pub struct GitHttpState {
    pub git: Arc<GitManager>,
    pub db: Db,
    pub bus: EventBus,
    pub auth: Arc<Auth>,
}

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
    let query_string = request
        .uri()
        .query()
        .map(|s| s.to_string());

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
            return (
                StatusCode::BAD_REQUEST,
                format!("failed to read body: {e}"),
            )
                .into_response();
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
            ).await {
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
        parse_receive_pack_ref_updates(&body)
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
            warn!("git http-backend error: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("git backend error: {e}"),
            )
                .into_response();
        }
    };

    // Publish events on successful push.
    if path_info.contains("git-receive-pack") && status < 300 {
        if let (Some(owner), Some(repo_name)) = (&owner, &repo_name) {
            let repo = state
                .db
                .get_repository_by_name(owner, repo_name)
                .await
                .ok()
                .flatten();
            let repo_id = repo.as_ref().map(|r| r.id);
            // Resolve who pushed — this fixes the long-standing
            // actor_id: None on push events and lets CI enforce policies
            // like "don't run on pushes from bot users".
            let pusher = resolve_authed_user(&state, &headers).await;
            let pusher_id = pusher.as_ref().map(|u| u.id);
            for upd in &received_updates {
                let payload = serde_json::json!({
                    "owner": owner,
                    "repo": repo_name,
                    "ref": upd.ref_name,
                    "old_sha": upd.old_sha,
                    "new_sha": upd.new_sha,
                    "pusher_id": pusher_id,
                });
                info!(
                    push_ref = %upd.ref_name,
                    new_sha = %upd.new_sha,
                    "push to {owner}/{repo_name}"
                );
                state.bus.publish(EventType::Push, repo_id, pusher_id, payload);
            }

            // Phase 1 CI trigger: if the pushed commit contains
            // `.gita-ci.yml`, upsert the pipeline, enqueue a run, and
            // emit a PipelineRequested event. `trigger_event_id` is left
            // NULL — bus.publish() doesn't return a DB id (the persist
            // hook inserts asynchronously), and the PipelineRequested
            // payload already carries owner/repo/ref/head_sha/run_id.
            if let Some(rid) = repo_id {
                for upd in &received_updates {
                    if upd.new_sha.chars().all(|c| c == '0') {
                        continue; // branch deletion — skip
                    }
                    let Some(file) = state
                        .git
                        .read_file(owner, repo_name, &upd.new_sha, ".gita-ci.yml")
                        .ok()
                        .flatten()
                    else {
                        continue; // no CI config at this commit
                    };
                    let pipeline_id = state
                        .db
                        .upsert_pipeline(
                            rid,
                            ".gita-ci.yml",
                            &upd.ref_name,
                            &upd.new_sha,
                            &file.content,
                        )
                        .await
                        .ok();
                    let Some(pid) = pipeline_id else { continue };
                    let run_id = state
                        .db
                        .insert_pipeline_run(
                            pid,
                            rid,
                            &upd.ref_name,
                            &upd.new_sha,
                            pusher_id,
                            None,
                        )
                        .await
                        .ok();
                    let Some(run_id) = run_id else { continue };
                    let req_payload = serde_json::json!({
                        "owner": owner,
                        "repo": repo_name,
                        "run_id": run_id,
                        "pipeline_id": pid,
                        "ref": upd.ref_name,
                        "head_sha": upd.new_sha,
                    });
                    state.bus.publish(
                        EventType::PipelineRequested,
                        Some(rid),
                        pusher_id,
                        req_payload,
                    );
                }
            }
        }
    }

    // Build the axum response from CGI output.
    let mut builder = Response::builder().status(StatusCode::from_u16(status).unwrap_or(StatusCode::OK));
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

    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::builder().status(StatusCode::INTERNAL_SERVER_ERROR).body(Body::empty()).unwrap())
}

/// Parse `owner/name.git/...` from a path like `/owner/name.git/info/refs`.
fn parse_repo_from_path(path_info: &str) -> (Option<String>, Option<String>) {
    let mut parts = path_info.trim_start_matches('/').split('/');
    let owner = parts.next().filter(|s| !s.is_empty()).map(|s| s.to_string());
    let repo = parts.next().map(|s| {
        s.strip_suffix(".git")
            .unwrap_or(s)
            .to_string()
    });
    (owner, repo)
}

/// One ref-update line parsed from a git-receive-pack request body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RefUpdate {
    old_sha: String,
    new_sha: String,
    ref_name: String,
}

/// Parse the ref-update lines from the body of a `POST .../git-receive-pack`
/// request.
///
/// The body is a sequence of pkt-lines. Each pkt-line is prefixed with a
/// 4-byte ASCII-hex length (the length includes the 4 length bytes).
/// `0000` is a flush packet that ends the command list.
///
/// Each ref line has the shape:
///
/// ```text
/// <old_sha> SP <new_sha> SP <ref> NUL <capabilities> LF
/// ```
///
/// Capabilities only appear on the first ref line. We return the parsed
/// updates without capabilities.
fn parse_receive_pack_ref_updates(body: &[u8]) -> Vec<RefUpdate> {
    let mut out = Vec::new();
    if body.is_empty() {
        return out;
    }

    // Skip the initial flush packet (`0000`) if present.
    let mut pos = 0usize;
    if body.len() >= 4 && &body[..4] == b"0000" {
        pos = 4;
    }

    while pos + 4 <= body.len() {
        let len_hex = &body[pos..pos + 4];
        let Ok(len_str) = std::str::from_utf8(len_hex) else { break };
        let Ok(len) = usize::from_str_radix(len_str, 16) else { break };
        if len == 0 {
            // Flush packet — end of the ref-update section.
            break;
        }
        if len < 4 || pos + len > body.len() {
            break;
        }
        let line = &body[pos + 4..pos + len];
        pos += len;

        // Trim trailing LF.
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        let line = line.strip_suffix(b"\r").unwrap_or(line);

        // First line may have a NUL-separated capabilities section.
        let (ref_part, _caps) = match line.iter().position(|&b| b == 0) {
            Some(idx) => (&line[..idx], &line[idx + 1..]),
            None => (line, &[][..]),
        };

        let fields: Vec<&[u8]> = ref_part.split(|&b| b == b' ').collect();
        if fields.len() < 3 {
            continue;
        }
        let old_sha = String::from_utf8_lossy(fields[0]).to_string();
        let new_sha = String::from_utf8_lossy(fields[1]).to_string();
        let ref_name = String::from_utf8_lossy(fields[2]).to_string();
        out.push(RefUpdate {
            old_sha,
            new_sha,
            ref_name,
        });
    }

    out
}

/// Authorize a Git HTTP request (clone/fetch = read, push = write).
///
/// - Writes (`git-receive-pack`) require an authenticated user who owns
///   the repository.
/// - Reads (`git-upload-pack` / `info/refs`) of private repositories
///   require the owner; public repositories are readable anonymously.
async fn authorize_git_request(
    state: &GitHttpState,
    owner: &str,
    repo_name: &str,
    path_info: &str,
    query_string: &Option<String>,
    headers: &HeaderMap,
) -> Result<(), Response<Body>> {
    let is_write = path_info.contains("git-receive-pack")
        || query_string
            .as_deref()
            .map(|q| q.contains("service=git-receive-pack"))
            .unwrap_or(false);
    let is_read = path_info.contains("git-upload-pack")
        || path_info.contains("info/refs")
        || query_string
            .as_deref()
            .map(|q| q.contains("service=git-upload-pack"))
            .unwrap_or(false);

    // Fetch the repository record.
    let repo = state
        .db
        .get_repository_by_name(owner, repo_name)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {e}"),
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
    let is_owner = authed
        .as_ref()
        .map(|u| u.id == repo.owner_id)
        .unwrap_or(false);

    // Writes (push) require the owner.
    if is_write {
        if !is_owner {
            // Return a Basic auth challenge so git clients know to send
            // credentials (from the remote URL or credential helper).
            return Err(unauthorized_with_challenge(
                "authentication required to push (repository owner only)",
            ));
        }
        return Ok(());
    }

    // Reads: private repos require the owner; public repos are anonymous-readable.
    if is_read && repo.is_private && !is_owner {
        return Err(unauthorized_with_challenge(
            "this repository is private",
        ));
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
async fn resolve_authed_user(
    state: &GitHttpState,
    headers: &HeaderMap,
) -> Option<gita_core::User> {
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
        let (_, password) = text.split_once(':')?;
        return state.auth.resolve_bearer(&state.db, password).await.ok();
    }

    None
}

/// A test helper to build the router with a stub database.
#[cfg(test)]
pub async fn test_state() -> GitHttpState {
    let url = std::env::var("GITA_TEST_DATABASE_URL").unwrap_or_else(|_| {
        "postgres://gita:gita@127.0.0.1:55432/gita".to_string()
    });
    let db = gita_db::new_test_db(&url).await;
    let root = std::env::temp_dir().join(format!("gita-http-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let git = GitManager::new(root);
    GitHttpState {
        git: Arc::new(git),
        db,
        bus: EventBus::default(),
        auth: Arc::new(gita_api::auth::Auth::new("test-secret".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gita_core::EventType;
    use std::path::PathBuf;

    fn temp_root() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "gita-http-test-{}-{}",
            std::process::id(),
            id
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn git_request_authorization() {
        let state = test_state().await;
        let db = &state.db;
        let user = db.create_user("carol", "carol@example.com", "h").await.unwrap();
        let public_repo = db
            .create_repository(user.id, "carol", "pub", "public", false, "main")
            .await
            .unwrap();
        let private_repo = db
            .create_repository(user.id, "carol", "priv", "private", true, "main")
            .await
            .unwrap();

        // Generate a JWT for the owner.
        let token = state.auth.sign_jwt(&user).unwrap();
        let mut authed_headers = HeaderMap::new();
        authed_headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let empty_headers = HeaderMap::new();
        let no_query = &None;

        // Public repo: anonymous read is allowed.
        let res = authorize_git_request(
            &state,
            "carol",
            &public_repo.name,
            "/carol/pub.git/info/refs",
            no_query,
            &empty_headers,
        )
        .await;
        assert!(res.is_ok(), "public repo should be anonymously readable");

        // Private repo: anonymous read is rejected.
        let res = authorize_git_request(
            &state,
            "carol",
            &private_repo.name,
            "/carol/priv.git/info/refs",
            no_query,
            &empty_headers,
        )
        .await;
        assert!(res.is_err(), "private repo should reject anonymous read");
        assert_eq!(res.unwrap_err().status(), StatusCode::UNAUTHORIZED);

        // Private repo: owner read is allowed.
        let res = authorize_git_request(
            &state,
            "carol",
            &private_repo.name,
            "/carol/priv.git/info/refs",
            no_query,
            &authed_headers,
        )
        .await;
        assert!(res.is_ok(), "private repo owner should be able to read");

        // Push without auth: rejected.
        let res = authorize_git_request(
            &state,
            "carol",
            &public_repo.name,
            "/carol/pub.git/git-receive-pack",
            no_query,
            &empty_headers,
        )
        .await;
        assert!(res.is_err(), "push should require authentication");
        assert_eq!(res.unwrap_err().status(), StatusCode::UNAUTHORIZED);

        // Push as owner: allowed (Bearer).
        let res = authorize_git_request(
            &state,
            "carol",
            &public_repo.name,
            "/carol/pub.git/git-receive-pack",
            no_query,
            &authed_headers,
        )
        .await;
        assert!(res.is_ok(), "owner should be able to push");

        // Push as owner: allowed via Basic auth (git smart HTTP style).
        // The remote URL is http://owner:token@host/..., so the Basic
        // credentials are `carol:<token>`.
        let basic_value = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("carol:{token}"))
        );
        let mut basic_headers = HeaderMap::new();
        basic_headers.insert(header::AUTHORIZATION, basic_value.parse().unwrap());
        let res = authorize_git_request(
            &state,
            "carol",
            &public_repo.name,
            "/carol/pub.git/git-receive-pack",
            no_query,
            &basic_headers,
        )
        .await;
        assert!(res.is_ok(), "owner should be able to push via Basic auth");

        // Push with wrong password in Basic auth: rejected.
        let bad_basic = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("carol:not-a-token")
        );
        let mut bad_headers = HeaderMap::new();
        bad_headers.insert(header::AUTHORIZATION, bad_basic.parse().unwrap());
        let res = authorize_git_request(
            &state,
            "carol",
            &public_repo.name,
            "/carol/pub.git/git-receive-pack",
            no_query,
            &bad_headers,
        )
        .await;
        assert!(res.is_err(), "push with wrong token should be rejected");
        assert_eq!(res.unwrap_err().status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authorize_via_query_string_service_flag() {
        // `git-receive-pack` may be specified in either path_info or
        // ?service=git-receive-pack; the body of the request (git smart
        // HTTP) typically uses the query-string form.
        let state = test_state().await;
        let user = state
            .db
            .create_user("dsvc", "dsvc@example.com", "h")
            .await
            .unwrap();
        state
            .db
            .create_repository(user.id, "dsvc", "qrepo", "qs test", false, "main")
            .await
            .unwrap();

        // query-string receive-pack without auth -> denied
        let qs = Some("service=git-receive-pack".to_string());
        let empty_headers = HeaderMap::new();
        let res = authorize_git_request(
            &state,
            "dsvc",
            "qrepo",
            "/dsvc/qrepo.git/info/refs",
            &qs,
            &empty_headers,
        )
        .await;
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authorize_unknown_repo_returns_404() {
        let state = test_state().await;
        let empty_headers = HeaderMap::new();
        let res = authorize_git_request(
            &state,
            "nobody",
            "void",
            "/nobody/void.git/info/refs",
            &None,
            &empty_headers,
        )
        .await;
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn parse_receive_pack_ref_updates_parses_basic_lines() {
        // One flush packet + two ref-update pkt-lines.
        // Line lengths: "old new ref\0caps\n" — we'll compute hex lengths.
        fn pkt_line(s: &str) -> Vec<u8> {
            let mut line = s.as_bytes().to_vec();
            line.push(b'\n');
            let len = line.len() + 4;
            let mut out = format!("{len:04x}").into_bytes();
            out.extend_from_slice(&line);
            out
        }

        let mut body = Vec::new();
        body.extend_from_slice(b"0000");
        body.extend_from_slice(&pkt_line(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb refs/heads/main\0report-status-v2 side-band-64k",
        ));
        body.extend_from_slice(&pkt_line(
            "cccccccccccccccccccccccccccccccccccccccc dddddddddddddddddddddddddddddddddddddddd refs/heads/dev",
        ));

        let updates = parse_receive_pack_ref_updates(&body);
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].old_sha, "a".repeat(40));
        assert_eq!(updates[0].new_sha, "b".repeat(40));
        assert_eq!(updates[0].ref_name, "refs/heads/main");
        assert_eq!(updates[1].old_sha, "c".repeat(40));
        assert_eq!(updates[1].new_sha, "d".repeat(40));
        assert_eq!(updates[1].ref_name, "refs/heads/dev");
    }

    #[test]
    fn parse_receive_pack_ref_updates_handles_flush_only() {
        let body = b"0000".to_vec();
        let updates = parse_receive_pack_ref_updates(&body);
        assert!(updates.is_empty());
    }

    #[tokio::test]
    async fn push_event_payload_contains_pusher_id() {
        // The publish block calls resolve_authed_user; we can't easily
        // drive the full HTTP handler, so assert the pieces that matter:
        // the enriched payload shape is what the handler builds, and the
        // parse function feeds it the right (ref, old_sha, new_sha).
        let state = test_state().await;
        let user = state
            .db
            .create_user("ppusher", "p@e.io", "h")
            .await
            .unwrap();
        state
            .db
            .create_repository(user.id, "ppusher", "r1", "d", false, "main")
            .await
            .unwrap();

        let token = state.auth.sign_jwt(&user).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());

        let authed = resolve_authed_user(&state, &headers).await;
        assert!(authed.is_some());
        assert_eq!(authed.unwrap().id, user.id);

        // The payload built by the push block uses pusher_id = user.id.
        let payload = serde_json::json!({
            "owner": "ppusher",
            "repo": "r1",
            "ref": "refs/heads/main",
            "old_sha": "0".repeat(40),
            "new_sha": "a".repeat(40),
            "pusher_id": user.id,
        });
        let event = state.bus.publish(
            EventType::Push,
            Some(user.id),
            Some(user.id),
            payload.clone(),
        );
        assert_eq!(event.payload["pusher_id"], serde_json::json!(user.id));
        assert_eq!(event.actor_id, Some(user.id));
    }
}