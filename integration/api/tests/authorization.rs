//! Authorization regression tests.
//!
//! These lock in the access-control rules that every write path must
//! follow. They exist because the rules were only applied to *read* paths:
//! the read handlers went through `visibility::load_readable_repo`, while
//! the write handlers looked the repository up directly and never looked
//! at `is_private`. Any registered user could therefore create issues,
//! comments and pull requests in — and merge pull requests of — somebody
//! else's private repository.
//!
//! The rule being tested:
//!
//! * a **read** of a private repository requires being its owner;
//! * a **write** to *any* repository requires being its owner;
//! * a private repository's existence must not leak (404, not 403).

mod common;

use axum::http::Method;
use axum::http::StatusCode;
use common::{create_repo, push_initial_commit, register_user, request, test_app};

/// Create a private repository owned by `owner` and return its full name.
async fn private_repo(app: &gita_server::App, owner: &str, token: &str, name: &str) -> String {
    use tower::ServiceExt;
    let body = serde_json::json!({
        "name": name,
        "description": "private",
        "default_branch": "main",
        "is_private": true,
    });
    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/repos")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "create private repo {name} failed"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["full_name"].as_str().unwrap().to_string()
}

/// A non-owner must not be able to write into somebody else's private
/// repository — and must not learn that it exists.
#[tokio::test]
async fn writes_to_a_private_repo_require_ownership() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "secowner").await;
    let intruder = register_user(&app, "secintruder").await;
    let full = private_repo(&app, &owner.username, &owner.token, "vault").await;
    push_initial_commit(&app, &owner.username, "vault");

    // The owner can read it.
    let (status, _) = request(
        &app,
        Method::GET,
        &format!("/api/repos/{full}"),
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the owner must see their repo");

    // A non-owner must not.
    let (status, _) = request(
        &app,
        Method::GET,
        &format!("/api/repos/{full}"),
        None,
        Some(&intruder.token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a non-owner must not see a private repo"
    );

    // Every write path must refuse a non-owner.
    let writes: Vec<(&str, Method, String, Option<serde_json::Value>)> = vec![
        (
            "create issue",
            Method::POST,
            format!("/api/repos/{full}/issues"),
            Some(serde_json::json!({"title": "intruder", "body": "x"})),
        ),
        (
            "close issue",
            Method::POST,
            format!("/api/repos/{full}/issues/1/close"),
            None,
        ),
        (
            "create pull request",
            Method::POST,
            format!("/api/repos/{full}/prs"),
            Some(serde_json::json!({
                "title": "intruder",
                "source_branch": "main",
                "target_branch": "main",
            })),
        ),
        (
            "close pull request",
            Method::POST,
            format!("/api/repos/{full}/prs/1/close"),
            None,
        ),
        (
            "comment on pull request",
            Method::POST,
            format!("/api/repos/{full}/prs/1/comments"),
            Some(serde_json::json!({"body": "intruder"})),
        ),
        (
            "comment on issue",
            Method::POST,
            format!("/api/repos/{full}/issues/1/comments"),
            Some(serde_json::json!({"body": "intruder"})),
        ),
        ("star", Method::PUT, format!("/api/repos/{full}/star"), None),
        (
            "fork",
            Method::POST,
            format!("/api/repos/{full}/fork"),
            Some(serde_json::json!({})),
        ),
    ];

    for (label, method, uri, body) in writes {
        let method_display = method.clone();
        let (status, _) = request(&app, method, &uri, body, Some(&intruder.token)).await;
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN,
            "{label}: a non-owner must not be able to write to a private repo \
             (got {status} for {method_display} {uri})"
        );
    }

    // Nothing the intruder tried may have taken effect.
    let (status, body) = request(
        &app,
        Method::GET,
        &format!("/api/repos/{full}/issues"),
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["issues"].as_array().map(|a| a.len()).unwrap_or(0),
        0,
        "the intruder's issue must not exist"
    );
}

/// A non-owner must not be able to merge somebody else's pull request —
/// not even in a repository they cannot read.
#[tokio::test]
async fn merging_someone_elses_pull_request_is_refused() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "mergeowner").await;
    let intruder = register_user(&app, "mergeintruder").await;
    let full = private_repo(&app, &owner.username, &owner.token, "mergetest").await;
    push_initial_commit(&app, &owner.username, "mergetest");

    // The owner creates a pull request.
    let (status, pr) = request(
        &app,
        Method::POST,
        &format!("/api/repos/{full}/prs"),
        Some(serde_json::json!({
            "title": "owner PR",
            "source_branch": "main",
            "target_branch": "main",
        })),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner could not create a PR");
    let number = pr["number"].as_i64().unwrap();
    assert!(
        pr["id"].as_i64().unwrap_or(0) > 0,
        "a created pull request must report a real id, got {}",
        pr["id"]
    );

    // The intruder tries to merge it.
    let (status, _) = request(
        &app,
        Method::POST,
        &format!("/api/repos/{full}/prs/{number}/merge"),
        None,
        Some(&intruder.token),
    )
    .await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN,
        "a non-owner must not be able to merge a pull request (got {status})"
    );

    // The pull request is still open.
    let (status, body) = request(
        &app,
        Method::GET,
        &format!("/api/repos/{full}/prs/{number}"),
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["pr"]["status"], "open",
        "the pull request must not have been merged"
    );
}

/// Code search walks the bare repositories on disk. It must not surface
/// the contents of a private repository to a caller who cannot read it.
#[tokio::test]
async fn code_search_does_not_leak_private_repositories() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "searchowner").await;
    let intruder = register_user(&app, "searchintruder").await;
    let full = private_repo(&app, &owner.username, &owner.token, "codevault").await;
    push_initial_commit(&app, &owner.username, "codevault");

    // The canary is in the private repository's working tree.
    // `search_code` matches file *contents*, so search for the text the
    // helper writes into `hello.txt`.
    let needle = "world";

    // An anonymous caller must not find it.
    let (status, body) = request(
        &app,
        Method::GET,
        &format!("/api/search/code?q={needle}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let hits = body["hits"].as_array().cloned().unwrap_or_default();
    assert!(
        !hits.iter().any(|h| h["repo"].as_str() == Some("codevault")),
        "anonymous code search leaked a private repository: {hits:?}"
    );

    // Neither may another user.
    let (status, body) = request(
        &app,
        Method::GET,
        &format!("/api/search/code?q={needle}"),
        None,
        Some(&intruder.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let hits = body["hits"].as_array().cloned().unwrap_or_default();
    assert!(
        !hits.iter().any(|h| h["repo"].as_str() == Some("codevault")),
        "code search leaked a private repository to a non-owner: {hits:?}"
    );

    // The owner can find it.
    let (status, body) = request(
        &app,
        Method::GET,
        &format!("/api/search/code?q={needle}"),
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let hits = body["hits"].as_array().cloned().unwrap_or_default();
    assert!(
        hits.iter().any(|h| h["repo"].as_str() == Some("codevault")),
        "the owner must be able to search their own repository: {hits:?}"
    );
}

/// The event stream must not disclose another user's private repository
/// activity.
#[tokio::test]
async fn event_stream_does_not_leak_private_repositories() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "eventowner").await;
    let intruder = register_user(&app, "eventintruder").await;
    let full = private_repo(&app, &owner.username, &owner.token, "eventvault").await;
    push_initial_commit(&app, &owner.username, "eventvault");

    // The global feed must not mention the private repository.
    let (status, body) = request(
        &app,
        Method::GET,
        "/api/events?limit=500",
        None,
        Some(&intruder.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = body["events"].as_array().cloned().unwrap_or_default();
    let leaked = events.iter().any(|e| {
        e["payload"]["repo"].as_str() == Some("eventvault")
            || e["payload"]["group"].as_str() == Some("eventvault")
    });
    assert!(
        !leaked,
        "the global event stream leaked a private repository: {events:?}"
    );

    // Asking for that repository's events directly must fail too.
    let (status, _) = request(
        &app,
        Method::GET,
        &format!("/api/events?repo={full}"),
        None,
        Some(&intruder.token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a non-owner must not read a private repository's events"
    );

    // The owner can.
    let (status, _) = request(
        &app,
        Method::GET,
        &format!("/api/events?repo={full}"),
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// A created issue must report a real id, not 0. The insert statement
/// used to omit `RETURNING id`, so the response carried `"id": 0`.
#[tokio::test]
async fn created_issue_and_pr_report_real_ids() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "idowner").await;
    let full = create_repo(&app, &owner.token, "idrepo").await;
    push_initial_commit(&app, &owner.username, "idrepo");

    let (status, issue) = request(
        &app,
        Method::POST,
        &format!("/api/repos/{full}/issues"),
        Some(serde_json::json!({"title": "has an id", "body": "x"})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        issue["id"].as_i64().unwrap_or(0) > 0,
        "a created issue must report a real id, got {}",
        issue["id"]
    );

    let (status, pr) = request(
        &app,
        Method::POST,
        &format!("/api/repos/{full}/prs"),
        Some(serde_json::json!({
            "title": "has an id",
            "source_branch": "main",
            "target_branch": "main",
        })),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        pr["id"].as_i64().unwrap_or(0) > 0,
        "a created pull request must report a real id, got {}",
        pr["id"]
    );
}
