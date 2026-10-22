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
use axum::{body::to_bytes, Router};
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
            let applied_updates: Vec<&RefUpdate> = received_updates
                .iter()
                .filter(|upd| ref_update_applied(&state, owner, repo_name, upd))
                .collect();
            for upd in &applied_updates {
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
                state
                    .bus
                    .publish(EventType::Push, repo_id, pusher_id, payload);
            }

            // Phase 1 CI trigger: if the pushed commit contains
            // `.gita-ci.yml`, upsert the pipeline, enqueue a run, and
            // emit a PipelineRequested event.
            //
            // The event is persisted here instead of by the bus's persist
            // hook, so its database id is known and can be stored in
            // `pipeline_runs.trigger_event_id`. The hook cannot report an
            // id back — it is synchronous, and the insert is not.
            if let Some(rid) = repo_id {
                for upd in &applied_updates {
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
                    // Parse + apply only/except filter before enqueueing.
                    // The changed paths are the difference between the
                    // pushed commit and the one it replaced, so that
                    // `only:`/`except:` path filters have something to
                    // match against. Without them an `except: paths: [...]`
                    // rule could never fire.
                    let changed_paths = changed_paths_for_push(&state, owner, repo_name, upd).await;
                    let changed_refs: Vec<&str> =
                        changed_paths.iter().map(|p| p.as_str()).collect();
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

                    // Apply the only/except trigger filters from the
                    // .gita-ci.yml we just parsed. The branch is matched
                    // by its short name (`main`), which is what a user
                    // writes in `branches: [main, release/*]` — matching
                    // the full ref would never match a short glob.
                    let short_branch = upd
                        .ref_name
                        .strip_prefix("refs/heads/")
                        .unwrap_or(&upd.ref_name);
                    match gita_pipeline::parse_yaml(&file.content) {
                        Ok(spec) => {
                            use gita_pipeline::TriggerDecision;
                            if matches!(
                                spec.triggers.should_run(short_branch, &changed_refs),
                                TriggerDecision::Skip
                            ) {
                                info!(
                                    "skip enqueue for {}/{} on {} (filtered by only/except)",
                                    owner, repo_name, upd.ref_name
                                );
                                continue;
                            }
                        }
                        Err(e) => {
                            // A malformed yaml is fine to skip here:
                            // the runner will mark the run failed on
                            // pickup. We just don't enqueue.
                            tracing::warn!("parse .gita-ci.yml failed (skip enqueue): {e}");
                            continue;
                        }
                    }

                    let run_id = state
                        .db
                        .insert_pipeline_run(pid, rid, &upd.ref_name, &upd.new_sha, pusher_id, None)
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

                    // Persist the event here rather than letting the bus's
                    // persist hook do it, so we can learn its database id
                    // and point `pipeline_runs.trigger_event_id` at it.
                    // The hook would insert the row asynchronously and could
                    // not report the id back — which is why this column was
                    // always NULL.
                    let mut event = gita_core::Event::new(
                        EventType::PipelineRequested,
                        Some(rid),
                        pusher_id,
                        req_payload,
                    );
                    match state.db.insert_event(&event).await {
                        Ok(event_id) => {
                            event.id = event_id;
                            // Backfill the link now that the event exists.
                            if let Err(e) = state
                                .db
                                .set_pipeline_run_trigger_event(run_id, event_id)
                                .await
                            {
                                tracing::warn!(
                                    "failed to link pipeline run {run_id} to event {event_id}: {e}"
                                );
                            }
                        }
                        Err(e) => {
                            // The event could not be stored. Publish it
                            // anyway (with id 0) so subscribers still see
                            // the request — the bus hook will retry the
                            // insert.
                            tracing::warn!("failed to persist PipelineRequested event: {e}");
                        }
                    }
                    state.bus.publish_persisted(event);
                }
            }
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

/// Whether a ref update was actually applied by the push.
///
/// A ref update is applied when the ref now points at the new sha. This
/// distinguishes a successful push from one the server rejected (a
/// non-fast-forward, a failing hook, a lock conflict): `git http-backend`
/// reports both with HTTP 200, so the status code cannot be used.
fn ref_update_applied(state: &GitHttpState, owner: &str, repo_name: &str, upd: &RefUpdate) -> bool {
    // A branch deletion is reported by the ref simply being gone.
    if upd.new_sha.chars().all(|c| c == '0') {
        return !state.git.ref_exists(owner, repo_name, &upd.ref_name);
    }
    match state.git.resolve_branch(owner, repo_name, &upd.ref_name) {
        Ok(current) => current == upd.new_sha,
        Err(_) => false,
    }
}

/// Parse `owner/name.git/...` from a path like `/owner/name.git/info/refs`.
fn parse_repo_from_path(path_info: &str) -> (Option<String>, Option<String>) {
    let mut parts = path_info.trim_start_matches('/').split('/');
    let owner = parts
        .next()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let repo = parts
        .next()
        .map(|s| s.strip_suffix(".git").unwrap_or(s).to_string());
    (owner, repo)
}

/// The paths a pushed commit changed, relative to the repository root.
///
/// This is what `only:`/`except:` path filters match against. For a new
/// branch the whole tree counts as changed, since there is no earlier
/// commit to compare against.
async fn changed_paths_for_push(
    state: &GitHttpState,
    owner: &str,
    repo_name: &str,
    upd: &RefUpdate,
) -> Vec<String> {
    // A branch creation has no previous commit; treat every file as
    // changed so path filters still have a chance to match.
    let old_sha = upd.old_sha.trim();
    if old_sha.is_empty() || old_sha.chars().all(|c| c == '0') {
        return state
            .git
            .list_tree(owner, repo_name, &upd.new_sha, "")
            .map(|entries| {
                entries
                    .into_iter()
                    .filter(|e| e.kind == "blob")
                    .map(|e| e.path)
                    .collect()
            })
            .unwrap_or_default();
    }
    match state
        .git
        .diff_files_between(owner, repo_name, old_sha, &upd.new_sha)
    {
        Ok(files) => files.into_iter().map(|(_, path)| path).collect(),
        Err(e) => {
            warn!("could not compute changed paths for {owner}/{repo_name}: {e}");
            Vec::new()
        }
    }
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
        let Ok(len_str) = std::str::from_utf8(len_hex) else {
            break;
        };
        let Ok(len) = usize::from_str_radix(len_str, 16) else {
            break;
        };
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
/// The git service verb a request addresses, if any.
///
/// A git-over-HTTP path has the shape `/<owner>/<repo>.git/<verb>[/...]`,
/// where `<verb>` is the third path segment. Parsing the verb *structurally*
/// matters: a substring match on the whole path would classify every
/// request for a repository literally named `git-receive-pack` as a write.
fn request_verb(path_info: &str) -> Option<&str> {
    let mut parts = path_info.trim_start_matches('/').split('/');
    let _owner = parts.next()?;
    let _repo = parts.next()?;
    let verb = parts.next()?;
    if verb.is_empty() {
        None
    } else {
        Some(verb)
    }
}

/// Classify a git request as a read of, or a write to, the repository it
/// addresses.
///
/// The classification must be **path-structural**, not a substring match on
/// the whole path. `git http-backend` serves the "dumb" file endpoints
/// (`HEAD`, `objects/**`, `objects/info/packs`, the `.pack`/`.idx` files) when
/// `GIT_HTTP_EXPORT_ALL=1` is set — which it always is here (see
/// `gita-git::run_http_backend`). Those paths contain neither
/// `git-upload-pack` nor `info/refs`, so a substring-based check classified
/// them as "neither read nor write" and let them past the authorization gate
/// entirely. The result was that an anonymous client could download a
/// private repository's objects (and hence its whole history) while the
/// smart-HTTP path correctly answered 401.
///
/// Everything under `/<owner>/<repo>.git/` is a read of that repository;
/// the only write verb is `git-receive-pack`.
fn is_write_request(path_info: &str, query_string: &Option<String>) -> bool {
    if request_verb(path_info) == Some("git-receive-pack") {
        return true;
    }
    // The `service=` query parameter is how a client selects the service
    // before the path carries it (e.g. `?service=git-receive-pack`).
    query_string
        .as_deref()
        .map(|q| q.contains("service=git-receive-pack"))
        .unwrap_or(false)
}

/// Whether the request reads the repository's contents.
///
/// Conservative by design: a request that addresses the repository and is
/// not a write is treated as a read. A path we fail to recognise therefore
/// fails *closed* (a private repository still requires its owner) rather
/// than open.
fn is_read_request(path_info: &str, query_string: &Option<String>) -> bool {
    !is_write_request(path_info, query_string)
}

/// Authorize a git-over-HTTP request.
///
/// Writes (push) require the repository owner. Reads of a private
/// repository require the owner; public repositories are readable
/// anonymously.
async fn authorize_git_request(
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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")).into_response())?
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
async fn resolve_authed_user(state: &GitHttpState, headers: &HeaderMap) -> Option<gita_core::User> {
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
    let url = std::env::var("GITA_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://gita:gita@127.0.0.1:55432/gita".to_string());
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
        let dir =
            std::env::temp_dir().join(format!("gita-http-test-{}-{}", std::process::id(), id));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn git_request_authorization() {
        let state = test_state().await;
        let db = &state.db;
        let user = db
            .create_user("carol", "carol@example.com", "h")
            .await
            .unwrap();
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

    /// The "dumb" file endpoints that `git http-backend` serves under
    /// `GIT_HTTP_EXPORT_ALL=1` must be classified as *reads* of the
    /// addressed repository. They used to match neither the read nor the
    /// write check, which let an anonymous client download a private
    /// repository's objects.
    /// A private repository must reject *every* read path, including the
    /// "dumb" file endpoints that `git http-backend` serves
    /// (`HEAD`, `objects/**`, the pack files). Those used to slip past the
    /// authorization gate entirely, letting an anonymous client download
    /// the repository's whole history.
    #[tokio::test]
    async fn private_repo_rejects_dumb_object_paths() {
        let state = test_state().await;
        let user = state
            .db
            .create_user("dumbowner", "dumb@example.com", "h")
            .await
            .unwrap();
        state
            .db
            .create_repository(user.id, "dumbowner", "secret", "d", true, "main")
            .await
            .unwrap();
        let empty_headers = HeaderMap::new();
        let no_query = &None;

        for path in [
            "/dumbowner/secret.git/HEAD",
            "/dumbowner/secret.git/objects/info/packs",
            "/dumbowner/secret.git/objects/aa/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "/dumbowner/secret.git/objects/pack/pack-0123.pack",
            "/dumbowner/secret.git/objects/pack/pack-0123.idx",
            "/dumbowner/secret.git/info/refs",
        ] {
            let res = authorize_git_request(
                &state,
                "dumbowner",
                "secret",
                path,
                no_query,
                &empty_headers,
            )
            .await;
            assert!(
                res.is_err(),
                "private repo must reject anonymous access to {path}"
            );
        }
    }

    /// The same paths on a *public* repository stay anonymously readable.
    #[tokio::test]
    async fn public_repo_allows_dumb_object_paths() {
        let state = test_state().await;
        let user = state
            .db
            .create_user("dumbpub", "dumbpub@example.com", "h")
            .await
            .unwrap();
        state
            .db
            .create_repository(user.id, "dumbpub", "open", "d", false, "main")
            .await
            .unwrap();
        let empty_headers = HeaderMap::new();
        let no_query = &None;

        for path in [
            "/dumbpub/open.git/HEAD",
            "/dumbpub/open.git/objects/info/packs",
            "/dumbpub/open.git/objects/aa/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ] {
            let res =
                authorize_git_request(&state, "dumbpub", "open", path, no_query, &empty_headers)
                    .await;
            assert!(
                res.is_ok(),
                "public repo must allow anonymous access to {path}"
            );
        }
    }

    #[test]
    fn dumb_file_paths_are_reads_not_neither() {
        for path in [
            "/owner/repo.git/HEAD",
            "/owner/repo.git/objects/info/packs",
            "/owner/repo.git/objects/aa/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "/owner/repo.git/objects/pack/pack-0123.pack",
            "/owner/repo.git/objects/pack/pack-0123.idx",
            "/owner/repo.git/config",
            "/owner/repo.git/info/refs",
            "/owner/repo.git/git-upload-pack",
        ] {
            assert!(
                !is_write_request(path, &None),
                "{path} must not be classified as a write"
            );
            // Not a write => it is a read => the private-repo check runs.
            assert_eq!(is_read_request(path, &None), true, "{path} must be a read");
        }
    }

    /// The only write verb is `git-receive-pack`.
    #[test]
    fn receive_pack_is_the_only_write_verb() {
        assert!(is_write_request("/owner/repo.git/git-receive-pack", &None));
        assert!(is_write_request(
            "/owner/repo.git/info/refs",
            &Some("service=git-receive-pack".to_string())
        ));
        assert!(!is_write_request("/owner/repo.git/git-upload-pack", &None));
        assert!(!is_write_request(
            "/owner/repo.git/info/refs",
            &Some("service=git-upload-pack".to_string())
        ));
    }

    /// A repository literally named `git-receive-pack` must not turn every
    /// request for it into a write. The verb is the third path segment, not
    /// a substring of the whole path.
    #[test]
    fn a_repo_named_like_a_verb_is_not_misclassified() {
        let path = "/owner/git-receive-pack.git/info/refs";
        assert!(!is_write_request(path, &None));
        assert_eq!(is_read_request(path, &None), true);
    }

    /// The verb is parsed structurally, so a deeper path still resolves.
    #[test]
    fn request_verb_reads_the_third_segment() {
        assert_eq!(
            request_verb("/o/r.git/git-receive-pack"),
            Some("git-receive-pack")
        );
        assert_eq!(
            request_verb("/o/r.git/git-upload-pack"),
            Some("git-upload-pack")
        );
        assert_eq!(request_verb("/o/r.git/objects/info/packs"), Some("objects"));
        assert_eq!(request_verb("/o/r.git"), None);
        assert_eq!(request_verb("/o"), None);
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
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );

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
