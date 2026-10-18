//! Shared helpers for API integration tests.
//!
//! `test_app()` boots a real `gita_server::App` against a Postgres
//! instance and returns the app plus a teardown guard. Tests should bind
//! the guard (e.g. `let _guard = guard;`) so cleanup fires at end of scope.
//!
//! `create_repo_with_commit()` creates a repo via the API and pushes a
//! real git commit so /api/repos/{owner}/{repo}/tree, /commits, the
//! `head_sha` lookup used by PR creation, etc., all return real data.

use std::path::Path;
use std::process::Command;

/// Database URL for integration tests. Override via `GITA_TEST_DATABASE_URL`.
pub fn test_db_url() -> String {
    std::env::var("GITA_TEST_DATABASE_URL").unwrap_or_else(|_| {
        "postgres://gita:gita@127.0.0.1:55432/gita".to_string()
    })
}

/// Holds the per-test schema and bookkeeping for best-effort cleanup.
pub struct TestAppGuard {
    pub base_url: String,
    pub schema: String,
    pub _tmp: tempfile::TempDir,
}

impl Drop for TestAppGuard {
    fn drop(&mut self) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let base_url = self.base_url.clone();
            let schema = self.schema.clone();
            handle.spawn(async move {
                if let Ok(pool) = sqlx::PgPool::connect(&base_url).await {
                    let _ = sqlx::query(&format!(
                        "DROP SCHEMA IF EXISTS \"{schema}\" CASCADE"
                    ))
                    .execute(&pool)
                    .await;
                    pool.close().await;
                }
            });
        }
    }
}

/// Build a fresh `App` with an isolated per-test Postgres schema.
pub async fn test_app() -> (gita_server::App, TestAppGuard) {
    use gita_server::Config;
    let tmp = tempfile::TempDir::new().unwrap();
    let schema = format!(
        "gita_it_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let base_url = test_db_url();
    {
        let pool = sqlx::PgPool::connect(&base_url).await.unwrap();
        sqlx::query(&format!("CREATE SCHEMA \"{schema}\""))
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    let mut config = Config::default();
    config.server.data_dir = tmp.path().join("data");
    config.server.database_url = base_url.clone();
    config.server.database_schema = Some(schema.clone());

    let app = gita_server::App::build(config).await.unwrap();

    let guard = TestAppGuard {
        base_url,
        schema,
        _tmp: tmp,
    };
    (app, guard)
}

/// Result of creating an authenticated test user via the running App.
pub struct AuthedUser {
    pub token: String,
    pub username: String,
}

/// Register a user via the App's HTTP layer and return a bearer token.
pub async fn register_user(
    app: &gita_server::App,
    username: &str,
) -> AuthedUser {
    use tower::ServiceExt;
    let body = serde_json::json!({
        "username": username,
        "email": format!("{username}@e2e.local"),
        "password": "hunter22",
    });
    let req = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri("/api/auth/register")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "register failed for {username}"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    AuthedUser {
        token: v["token"].as_str().unwrap().to_string(),
        username: username.to_string(),
    }
}

/// Create a repository for an authenticated user and return its `owner/name`.
pub async fn create_repo(app: &gita_server::App, token: &str, name: &str) -> String {
    use tower::ServiceExt;
    let body = serde_json::json!({
        "name": name,
        "description": "integration test repo",
        "default_branch": "main",
    });
    let req = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri("/api/repos")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "create_repo {name} failed"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["full_name"].as_str().unwrap().to_string()
}

/// Push a single commit into the bare repository located at
/// `<data_dir>/repos/<owner>/<name>.git` so that endpoints reading git
/// history (tree, commits, head_sha for PR creation, etc.) see real data.
///
/// Creates an isolated work tree under `data_dir/.work-<rand>`, commits a
/// README + an extra file, then pushes to origin/main.
pub fn push_initial_commit(app: &gita_server::App, owner: &str, name: &str) {
    let bare = app.data_dir.join("repos").join(owner).join(format!("{name}.git"));
    if !bare.exists() {
        panic!(
            "bare repo path not found: {} (data_dir={})",
            bare.display(),
            app.data_dir.display()
        );
    }
    let bare_url = format!("file://{}", bare.display());

    // Work in a sibling dir; cleanup via RAII temp dir.
    let work_parent = app.data_dir.join(format!(".work-{}", std::process::id()));
    std::fs::create_dir_all(&work_parent).unwrap();
    let work_root = work_parent.join(format!("wktmp-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&work_root).unwrap();

    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(&work_root)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {:?} failed:\nstdout: {}\nstderr: {}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    };

    git(&["init", "-q", "-b", "main"]);
    std::fs::write(work_root.join("README.md"), "# Hello\n").unwrap();
    std::fs::write(work_root.join("hello.txt"), "world\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "initial commit"]);
    git(&["remote", "add", "origin", &bare_url]);
    git(&["push", "-q", "-u", "origin", "main"]);
}

/// Drop straggler test schemas left over from interrupted runs.
/// Marked `#[ignore]` so it isn't run on every `cargo test`; invoke with
/// `cargo test -p gita-integration-api -- --ignored cleanup` to clean up.
#[tokio::test]
#[ignore = "manual cleanup of stale test schemas; run with --ignored"]
pub async fn cleanup_test_schemas() {
    let pool = sqlx::PgPool::connect(&test_db_url()).await.unwrap();
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT schema_name FROM information_schema.schemata \
         WHERE schema_name LIKE 'gita_it_%' \
            OR schema_name LIKE 'gita_test_%' \
            OR schema_name LIKE 'gita_schema_probe%' \
            OR schema_name LIKE 'gita_probe%'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for (s,) in rows {
        let _ = sqlx::query(&format!("DROP SCHEMA IF EXISTS \"{s}\" CASCADE"))
            .execute(&pool)
            .await;
    }
}

/// Helper used by individual test files to issue an HTTP request via the
/// App's router. Returns `(status_code, json_value)`.
pub async fn request(
    app: &gita_server::App,
    method: axum::http::Method,
    uri: &str,
    body: Option<serde_json::Value>,
    token: Option<&str>,
) -> (axum::http::StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let mut builder = axum::http::Request::builder().method(method).uri(uri);
    if let Some(tok) = token {
        builder = builder.header("authorization", format!("Bearer {tok}"));
    }
    let req = match body {
        Some(b) => builder
            .header("content-type", "application/json")
            .body(axum::body::Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap(),
        None => builder.body(axum::body::Body::empty()).unwrap(),
    };
    let resp = app.router().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, value)
}

/// Convenience: require that a path exists on disk (used to confirm the
/// git CLI created the expected work directory).
pub fn ensure_exists(p: &Path) {
    assert!(
        p.exists(),
        "expected path {} to exist",
        p.display()
    );
}
