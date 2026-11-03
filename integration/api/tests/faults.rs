//! Fault-injection tests: the server must fail closed and fail *quietly*.
//!
//! A database that goes away is the most common production failure. The
//! server must keep answering (so a load balancer can see it), report the
//! degradation, and never turn an internal error into a response that leaks
//! file paths, table names or SQL.

mod common;

use axum::http::Method;
use std::time::Duration;

/// Close the application's pool and wait until it is really closed, so the
/// next query fails instead of racing a shutdown.
async fn cut_the_database(app: &gita_admin::App) {
    let closed = tokio::time::timeout(Duration::from_secs(5), app.db.pool().close()).await;
    assert!(
        closed.is_ok(),
        "the test could not close the pool; the fault was not injected"
    );
}

/// Health stays reachable and names the broken dependency instead of
/// returning a bare 500.
#[tokio::test]
async fn health_reports_a_degraded_database_instead_of_failing() {
    let (app, _guard) = common::test_app().await;
    cut_the_database(&app).await;

    let (status, body) = common::request(&app, Method::GET, "/api/health", None, None).await;

    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["status"], "degraded");
    assert_eq!(body["db"], "error");
    assert!(body["git"].is_string());
}

/// A query failure becomes a generic 500. The body must not echo the SQLx
/// error, which would disclose the schema and connection details.
#[tokio::test]
async fn a_database_failure_does_not_leak_the_internal_error() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "faultuser").await;
    cut_the_database(&app).await;

    let (status, body) =
        common::request(&app, Method::GET, "/api/repos", None, Some(&user.token)).await;

    assert_eq!(
        status,
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "a broken database must be a 500, got {body}"
    );
    assert_eq!(body["error"]["message"], "internal error");
    assert_eq!(body["error"]["status"], 500);

    let rendered = body.to_string().to_lowercase();
    for leaked in [
        "postgres",
        "sqlx",
        "pool",
        "relation",
        "column",
        "gita_it_",
        "/repos/",
        "database_url",
    ] {
        assert!(
            !rendered.contains(leaked),
            "the 500 body leaked `{leaked}`: {body}"
        );
    }
}
