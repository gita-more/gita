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
use gita_ai::AiEngine;
use gita_api::auth::Auth;
use gita_core::{Event, EventType};
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
    pub ai: Arc<AiEngine>,
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
            let payload = serde_json::json!({
                "owner": owner,
                "repo": repo_name,
                "push": true,
            });
            info!("push to {owner}/{repo_name}");
            state.bus.publish(EventType::Push, repo_id, None, payload);

            // Automatically trigger an AI review of the pushed changes.
            // Spawn a background task so the push response is not blocked.
            if let (Some(repo), Some(repo_id)) = (repo, repo_id) {
                let ai = state.ai.clone();
                let git = state.git.clone();
                let db = state.db.clone();
                let bus = state.bus.clone();
                let owner = owner.clone();
                let repo_name = repo_name.clone();
                tokio::spawn(async move {
                    if let Err(e) = run_auto_review(&db, &git, &ai, &bus, &owner, &repo_name, repo_id, repo.default_branch).await {
                        warn!("auto review failed for {owner}/{repo_name}: {e}");
                    }
                });
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

/// Run an AI review of the changes pushed to `branch`.
///
/// The diff is computed between the latest previously-reviewed commit
/// (if any) and the current HEAD of the branch. If no review exists yet,
/// the diff is computed against the empty tree (initial full review).
async fn run_auto_review(
    db: &Db,
    git: &GitManager,
    ai: &AiEngine,
    bus: &EventBus,
    owner: &str,
    repo_name: &str,
    repo_id: i64,
    branch: String,
) -> anyhow::Result<()> {
    // Resolve the current HEAD of the default branch.
    let head_sha = git.head_sha(owner, repo_name, &branch)?;

    // Find the latest previously-reviewed commit SHA for this repo.
    let prev_sha = db
        .list_reviews(Some(repo_id), None, 1)
        .await?
        .into_iter()
        .next()
        .map(|r| r.commit_sha);

    // If the latest review already covers the current HEAD, skip.
    if prev_sha.as_deref() == Some(head_sha.as_str()) {
        debug!("no new commits since last review; skipping auto review");
        return Ok(());
    }

    // Compute the diff: either from the last reviewed commit, or from the
    // empty tree when this is the first review of the repository.
    let base = prev_sha.unwrap_or_else(|| empty_tree_sha().to_string());
    let diff = git.diff_between(owner, repo_name, &base, &head_sha)?;

    // Run the LLM review.
    let result = ai.review_diff(&diff, &format!("{owner}/{repo_name}")).await?;

    // Persist the review.
    let review = gita_ai::review_from_result(repo_id, None, &head_sha, result);
    let review_id = db.insert_review(&review).await?;

    info!(
        "auto review {review_id} completed for {owner}/{repo_name} at {head_sha} (risk={})",
        review.risk_score
    );

    bus.publish(
        EventType::ReviewCompleted,
        Some(repo_id),
        None,
        serde_json::json!({
            "repo": repo_name,
            "review_id": review_id,
            "commit_sha": head_sha,
            "risk_score": review.risk_score,
        }),
    );
    Ok(())
}

/// The SHA of the empty git tree object.
fn empty_tree_sha() -> &'static str {
    "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
}

/// A test helper to build the router with a stub database.
#[cfg(test)]
pub async fn test_state() -> GitHttpState {
    let db = gita_db::new_test_db().await;
    let root = std::env::temp_dir().join(format!("gita-http-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let git = GitManager::new(root);
    GitHttpState {
        git: Arc::new(git),
        db,
        bus: EventBus::default(),
        ai: Arc::new(gita_ai::AiEngine::stub()),
        auth: Arc::new(gita_api::auth::Auth::new("test-secret".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gita_core::EventType;
    use std::path::PathBuf;
    use std::process::Command;

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

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Run a git command in `cwd`, panicking on failure.
    fn git_run(cwd: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .expect("failed to spawn git");
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            panic!("git {:?} failed\nstdout: {}\nstderr: {}", args, stdout, stderr);
        }
    }

    /// Push an initial commit to the bare repo so it has a `main` branch.
    fn push_initial_commit(git: &GitManager, owner: &str, name: &str) {
        let repo_path = git.repo_path(owner, name);
        let work = repo_path.parent().unwrap().join("work");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();

        git_run(&work, &["init", "-b", "main"]);
        std::fs::write(work.join("README.md"), "# Demo\n\nAuto review test.\n").unwrap();
        git_run(&work, &["add", "."]);
        git_run(&work, &[
            "-c", "user.email=test@example.com",
            "-c", "user.name=Test",
            "commit", "-m", "initial commit",
        ]);
        let url = format!("file://{}", repo_path.to_string_lossy());
        git_run(&work, &["remote", "add", "origin", &url]);
        git_run(&work, &["push", "-u", "origin", "main"]);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[tokio::test]
    async fn push_auto_review_creates_review_and_event() {
        if !git_available() {
            eprintln!("git not available, skipping");
            return;
        }
        let db = gita_db::new_test_db().await;
        let user = db.create_user("alice", "a@b.c", "h").await.unwrap();
        let repo = db
            .create_repository(user.id, "alice", "demo", "d", false, "main")
            .await
            .unwrap();

        let root = temp_root();
        let git = GitManager::new(root.clone());
        git.create_repo("alice", "demo", "main").await.unwrap();
        push_initial_commit(&git, "alice", "demo");

        let ai = gita_ai::AiEngine::stub();
        let bus = EventBus::default();
        let mut rx = bus.subscribe();

        run_auto_review(&db, &git, &ai, &bus, "alice", "demo", repo.id, "main".to_string())
            .await
            .unwrap();

        // The review should be persisted.
        let reviews = db.list_reviews(Some(repo.id), None, 10).await.unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].repo_id, repo.id);
        assert!(!reviews[0].commit_sha.is_empty());
        assert!(reviews[0].risk_score >= 0);
        assert_eq!(reviews[0].status, "completed");

        // A ReviewCompleted event should be published.
        let ev = rx.recv().await.expect("event should be published");
        assert_eq!(ev.event_type, EventType::ReviewCompleted);
        assert_eq!(ev.payload["review_id"].as_i64(), Some(reviews[0].id));
        assert_eq!(ev.payload["commit_sha"].as_str(), Some(reviews[0].commit_sha.as_str()));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn push_auto_review_skips_when_no_new_commits() {
        if !git_available() {
            eprintln!("git not available, skipping");
            return;
        }
        let db = gita_db::new_test_db().await;
        let user = db.create_user("bob", "b@b.c", "h").await.unwrap();
        let repo = db
            .create_repository(user.id, "bob", "demo2", "d", false, "main")
            .await
            .unwrap();

        let root = temp_root();
        let git = GitManager::new(root.clone());
        git.create_repo("bob", "demo2", "main").await.unwrap();
        push_initial_commit(&git, "bob", "demo2");

        let ai = gita_ai::AiEngine::stub();
        let bus = EventBus::default();

        // First review.
        run_auto_review(&db, &git, &ai, &bus, "bob", "demo2", repo.id, "main".to_string())
            .await
            .unwrap();
        assert_eq!(db.list_reviews(Some(repo.id), None, 10).await.unwrap().len(), 1);

        // Second call with no new commits: should be a no-op.
        run_auto_review(&db, &git, &ai, &bus, "bob", "demo2", repo.id, "main".to_string())
            .await
            .unwrap();
        assert_eq!(db.list_reviews(Some(repo.id), None, 10).await.unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&root);
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
}