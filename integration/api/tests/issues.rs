mod common;

use common::{register_user, request};

#[tokio::test]
async fn issue_full_lifecycle() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "issue-user").await;
    let full = common::create_repo(&app, &user.token, "issuebox").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);

    let uri_issues = format!("/api/repos/{owner}/{name}/issues");
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &uri_issues,
        Some(serde_json::json!({"title": "found a bug", "body": "details"})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create: {body}");
    let issue_number = body["number"].as_i64().unwrap();

    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &uri_issues,
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["issues"].as_array().unwrap().len(), 1);

    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("{uri_issues}/{issue_number}"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["status"], "open");

    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        &format!("{uri_issues}/{issue_number}/close"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // status filter shows closed
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("{uri_issues}?status=closed"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["issues"].as_array().unwrap().len(), 1);

    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        &format!("{uri_issues}/{issue_number}/close"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CONFLICT);

    // Empty title rejected
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        &uri_issues,
        Some(serde_json::json!({"title": "   ", "body": "x"})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn issue_comments_lifecycle() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "cmt-issue-user").await;
    let full = common::create_repo(&app, &user.token, "cmt-issue").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);

    let uri_issues = format!("/api/repos/{owner}/{name}/issues");
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &uri_issues,
        Some(serde_json::json!({"title": "bug", "body": "details"})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create issue: {body}");
    let issue_number = body["number"].as_i64().unwrap();

    let uri_comments = format!("/api/repos/{owner}/{name}/issues/{issue_number}/comments");
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &uri_comments,
        Some(serde_json::json!({"body": "I can repro"})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create comment: {body}");
    let comment_id = body["id"].as_i64().unwrap();

    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &uri_comments,
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let comments = body["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0]["body"], "I can repro");

    // A second user cannot delete another user's comment.
    let intruder = register_user(&app, "cmt-intruder").await;
    let (status, _) = request(
        &app,
        axum::http::Method::DELETE,
        &format!("/api/comments/{comment_id}"),
        None,
        Some(&intruder.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    // Author can delete.
    let (status, _) = request(
        &app,
        axum::http::Method::DELETE,
        &format!("/api/comments/{comment_id}"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
}
