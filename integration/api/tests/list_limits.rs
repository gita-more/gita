//! The `limit` of the list endpoints.
//!
//! Every list endpoint caps how many rows it returns, so a caller
//! cannot pull the whole table in one request. The caps were never
//! asserted: a regression that dropped a cap — or quietly changed a
//! default — would not have been noticed by any test.

mod common;

use axum::http::{Method, StatusCode};
use common::{create_repo, register_user, request, test_app};

/// Create `count` issues in a repository.
///
/// Most issues are inserted straight into the database: a list-limit test
/// needs hundreds of rows, and 205 HTTP round trips would dominate the
/// suite's runtime. One issue still goes through the HTTP API, so the
/// row shape the endpoint writes is what the list reads.
async fn create_issues(app: &gita_server::App, token: &str, owner: &str, name: &str, count: usize) {
    let (status, body) = request(
        app,
        Method::POST,
        &format!("/api/repos/{owner}/{name}/issues"),
        Some(serde_json::json!({"title": "issue 0"})),
        Some(token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create issue 0: {body}");

    let repo = app
        .db
        .get_repository_by_name(owner, name)
        .await
        .expect("repo lookup")
        .expect("the repository exists");
    for i in 1..count {
        app.db
            .create_issue(repo.id, &format!("issue {i}"), "", repo.owner_id, "test")
            .await
            .expect("create issue");
    }
}

/// The number of rows a list endpoint returned.
fn count(body: &serde_json::Value) -> usize {
    body["issues"]
        .as_array()
        .or_else(|| body["events"].as_array())
        .expect("a list")
        .len()
}

/// The repository issue list caps at 200 and defaults to 30.
#[tokio::test]
async fn the_repository_issue_list_is_capped() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "limituser").await;
    create_repo(&app, &user.token, "issues").await;
    create_issues(&app, &user.token, "limituser", "issues", 205).await;

    // The default is 30.
    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/limituser/issues/issues",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "issue list: {body}");
    assert_eq!(count(&body), 30, "the default limit is 30: {body}");

    // An explicit limit is honoured.
    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/limituser/issues/issues?limit=5",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "issue list: {body}");
    assert_eq!(count(&body), 5, "an explicit limit is honoured: {body}");

    // A limit beyond the cap is clamped to the cap.
    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/limituser/issues/issues?limit=100000",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "issue list: {body}");
    assert_eq!(count(&body), 200, "the limit is capped at 200: {body}");
}

/// The global issue list caps at 200 and defaults to 50.
#[tokio::test]
async fn the_global_issue_list_is_capped() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "globallimit").await;
    create_repo(&app, &user.token, "issues").await;
    create_issues(&app, &user.token, "globallimit", "issues", 205).await;

    let (status, body) = request(&app, Method::GET, "/api/issues", None, Some(&user.token)).await;
    assert_eq!(status, StatusCode::OK, "issue list: {body}");
    assert_eq!(count(&body), 50, "the default limit is 50: {body}");

    let (status, body) = request(
        &app,
        Method::GET,
        "/api/issues?limit=100000",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "issue list: {body}");
    assert_eq!(count(&body), 200, "the limit is capped at 200: {body}");
}

/// The event log caps at 500 and defaults to 50.
#[tokio::test]
async fn the_event_list_is_capped() {
    use gita_core::{Event, EventType};

    let (app, _guard) = test_app().await;
    let user = register_user(&app, "eventlimit").await;
    create_repo(&app, &user.token, "events").await;
    let repo = app
        .db
        .get_repository_by_name("eventlimit", "events")
        .await
        .expect("repo lookup")
        .expect("the repository exists");

    // Overflow the default of 50 with plain push events.
    for _ in 0..60 {
        app.db
            .insert_event(&Event::new(
                EventType::Push,
                Some(repo.id),
                Some(repo.owner_id),
                serde_json::json!({}),
            ))
            .await
            .expect("insert event");
    }

    let (status, body) = request(&app, Method::GET, "/api/events", None, Some(&user.token)).await;
    assert_eq!(status, StatusCode::OK, "event list: {body}");
    assert_eq!(count(&body), 50, "the default limit is 50: {body}");

    let (status, body) = request(
        &app,
        Method::GET,
        "/api/events?limit=7",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "event list: {body}");
    assert_eq!(count(&body), 7, "an explicit limit is honoured: {body}");
}
