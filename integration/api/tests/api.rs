//! API integration tests.
//!
//! Boots the full `gita_server::App` against a real Postgres and issues HTTP
//! requests through `axum::Router::oneshot` — no listening socket, no race
//! between concurrent tests.
//!
//! Each test gets its own Postgres schema (`gita_it_<pid>_<n>`) for
//! isolation. `TestAppGuard` schedules schema cleanup on Drop using
//! `tokio::task::spawn`, so a guard simply needs to be bound (e.g.
//! `let _guard = guard;`) to fire the teardown at end of scope.

use gita_server::Config;

/// Database URL for integration tests. Override via `GITA_TEST_DATABASE_URL`.
fn test_db_url() -> String {
    std::env::var("GITA_TEST_DATABASE_URL").unwrap_or_else(|_| {
        "postgres://gita:gita@127.0.0.1:55432/gita".to_string()
    })
}

/// Holds the per-test schema and bookkeeping for best-effort cleanup.
struct TestAppGuard {
    base_url: String,
    schema: String,
}

impl Drop for TestAppGuard {
    fn drop(&mut self) {
        // We're being dropped (most likely at end of a tokio test), so we
        // can borrow the current runtime. If for some reason there is no
        // runtime here we silently skip cleanup — the schema will be
        // garbage-collected by `cleanup_test_schemas` (an --ignored test)
        // on a subsequent run.
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

/// Build a fresh `App` for testing with an isolated per-test schema.
async fn test_app() -> (gita_server::App, TestAppGuard) {
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
    // Materialize the schema in a separate connection; main pool will
    // pick it up via connect_with_schema.
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
    };
    (app, guard)
}

// `test_db_url()` / `test_app()` helpers above create per-test schemas;
// the cleanup below sweeps the database on demand.
#[allow(dead_code)]
async fn cleanup_test_schemas_impl() {
    let pool = sqlx::PgPool::connect(&test_db_url()).await.unwrap();
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT schema_name FROM information_schema.schemata WHERE schema_name LIKE 'gita_it_%' OR schema_name LIKE 'gita_test_%' OR schema_name LIKE 'gita_%' AND schema_name LIKE 'gita_diag_%'")
            .fetch_all(&pool)
            .await
            .unwrap();
    for (s,) in rows {
        let _ = sqlx::query(&format!("DROP SCHEMA IF EXISTS \"{s}\" CASCADE"))
            .execute(&pool)
            .await;
    }
}

/// Drop straggler test schemas left over from interrupted runs.
/// Marked `#[ignore]` so it isn't run on every `cargo test`; invoke with
/// `cargo test -p gita-integration-api -- --ignored cleanup` to clean up.
#[tokio::test]
#[ignore = "manual cleanup of stale test schemas; run with --ignored"]
async fn cleanup_test_schemas() {
    cleanup_test_schemas_impl().await;
}

async fn request(
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

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_then_login_then_me() {
    let (app, _guard) = test_app().await;

    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/register",
        Some(serde_json::json!({
            "username": "alice",
            "email": "alice@example.com",
            "password": "hunter22",
        })),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "register: {body}");
    let token = body["token"].as_str().unwrap().to_string();
    assert_eq!(body["user"]["username"], "alice");

    // duplicate username -> 409
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/register",
        Some(serde_json::json!({
            "username": "alice",
            "email": "other@example.com",
            "password": "hunter22",
        })),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CONFLICT);

    // login
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/login",
        Some(serde_json::json!({
            "username": "alice",
            "password": "hunter22",
        })),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "login: {body}");
    assert_eq!(body["user"]["username"], "alice");

    // me with token
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/auth/me",
        None,
        Some(&token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "me: {body}");
    assert_eq!(body["username"], "alice");
}

#[tokio::test]
async fn register_validation_errors() {
    let (app, _guard) = test_app().await;

    // short password
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/register",
        Some(serde_json::json!({
            "username": "bob",
            "email": "bob@example.com",
            "password": "123",
        })),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);

    // bad email
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/register",
        Some(serde_json::json!({
            "username": "bob",
            "email": "not-an-email",
            "password": "hunter22",
        })),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn login_wrong_password_returns_401() {
    let (app, _guard) = test_app().await;

    let _ = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/register",
        Some(serde_json::json!({
            "username": "carol",
            "email": "carol@example.com",
            "password": "hunter22",
        })),
        None,
    )
    .await;

    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/login",
        Some(serde_json::json!({
            "username": "carol",
            "password": "wrongpass",
        })),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Repos
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_repo_and_list() {
    let (app, _guard) = test_app().await;

    let (_, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/register",
        Some(serde_json::json!({
            "username": "dave",
            "email": "dave@example.com",
            "password": "hunter22",
        })),
        None,
    )
    .await;
    let token = body["token"].as_str().unwrap().to_string();

    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/repos",
        Some(serde_json::json!({
            "name": "hello",
            "description": "A test repo",
            "default_branch": "main",
        })),
        Some(&token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create repo: {body}");
    assert_eq!(body["owner"], "dave");
    assert_eq!(body["name"], "hello");
    assert_eq!(body["full_name"], "dave/hello");

    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/repos",
        None,
        Some(&token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let arr = body.as_array().unwrap();
    assert!(arr.iter().any(|r| r["name"] == "hello"));
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_reports_ok() {
    let (app, _guard) = test_app().await;

    let (status, body) =
        request(&app, axum::http::Method::GET, "/api/health", None, None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["db"], "ok");
}
