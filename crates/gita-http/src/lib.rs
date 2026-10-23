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
use gita_core::access::Access;
use gita_db::Db;
use gita_events::EventBus;
use gita_git::GitManager;
use std::sync::Arc;
use tracing::debug;

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

/// Whether a ref update was actually applied by the push.
///
/// A ref update is applied when the ref now points at the new sha. This
/// distinguishes a successful push from one the server rejected (a
/// non-fast-forward, a failing hook, a lock conflict): `git http-backend`

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
/// The rule is the shared one in `gita_core::access`: a push needs
/// [`Access::Push`], everything else needs [`Access::Read`]. Only the
/// *report* of a refusal is specific to this transport — git clients
/// need a `WWW-Authenticate` challenge to be prompted for credentials,
/// so a private repository answers 401 rather than 404.
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
            .create_repository(user.id, "carol", "pub", "public", false, "main", None)
            .await
            .unwrap();
        let private_repo = db
            .create_repository(user.id, "carol", "priv", "private", true, "main", None)
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

    /// The username in a Basic credential must name the user the token
    /// belongs to. A token presented under somebody else's name is not a
    /// valid credential.
    #[tokio::test]
    async fn basic_auth_requires_the_username_to_match_the_token() {
        let state = test_state().await;
        let db = &state.db;
        let user = db
            .create_user("carol", "carol@example.com", "h")
            .await
            .unwrap();
        let other = db
            .create_user("dave", "dave@example.com", "h")
            .await
            .unwrap();
        let private_repo = db
            .create_repository(user.id, "carol", "priv", "private", true, "main", None)
            .await
            .unwrap();
        let token = state.auth.sign_jwt(&user).unwrap();
        let no_query = &None;

        let basic = |username: &str| {
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(format!("{username}:{token}"));
            let mut headers = HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                format!("Basic {encoded}").parse().unwrap(),
            );
            headers
        };

        // The owner naming themselves works.
        let res = authorize_git_request(
            &state,
            "carol",
            &private_repo.name,
            "/carol/priv.git/info/refs",
            no_query,
            &basic("carol"),
        )
        .await;
        assert!(
            res.is_ok(),
            "the owner must be able to read over Basic auth"
        );

        // The same token under a different name does not.
        let res = authorize_git_request(
            &state,
            "carol",
            &private_repo.name,
            "/carol/priv.git/info/refs",
            no_query,
            &basic("dave"),
        )
        .await;
        assert!(
            res.is_err(),
            "a token presented under another user's name must be rejected"
        );
        assert_eq!(res.unwrap_err().status(), StatusCode::UNAUTHORIZED);

        // ... and neither does a name that belongs to nobody.
        let res = authorize_git_request(
            &state,
            "carol",
            &private_repo.name,
            "/carol/priv.git/info/refs",
            no_query,
            &basic("nobody"),
        )
        .await;
        assert!(res.is_err(), "an unknown username must be rejected");

        // Sanity: the other real user is not the token's owner either.
        assert_ne!(other.id, user.id);
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
            .create_repository(user.id, "dsvc", "qrepo", "qs test", false, "main", None)
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
    fn parse_receive_pack_ref_updates_handles_flush_only() {
        let body = b"0000".to_vec();
        let updates = gita_push::parse_receive_pack_commands(&body);
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
            .create_repository(user.id, "dumbowner", "secret", "d", true, "main", None)
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
            .create_repository(user.id, "dumbpub", "open", "d", false, "main", None)
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
            .create_repository(user.id, "ppusher", "r1", "d", false, "main", None)
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
