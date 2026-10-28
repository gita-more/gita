//! Repositories whose names collide with a sub-resource segment.
//!
//! `/api/repos/{owner}/{repo}` is served by one catch-all dispatcher
//! that dispatches on the *trailing* path segment: `.../tree` is a tree
//! listing, `.../dir` a directory listing, `.../prs` the pull requests.
//!
//! That made a repository named `dir`, `file`, `tree`, `hooks`, …
//! unreachable: asking for its metadata was answered with the
//! sub-resource instead — or with a 400 — so the repository looked
//! broken even though it existed and could be pushed to.
//!
//! The dispatcher now resolves the ambiguity against the database: the
//! sub-resource interpretation only wins when the repository it points
//! at really exists; otherwise the whole path names the repository.

mod common;

use axum::http::{Method, StatusCode};
use common::{create_repo, register_user, request, test_app};

/// Every segment the repository dispatcher treats as a sub-resource.
const RESERVED: &[&str] = &[
    "dir",
    "tree",
    "file",
    "commits",
    "branches",
    "tags",
    "star",
    "hooks",
    "prs",
    "issues",
    "pipelines",
    "runs",
    "pages",
];

/// A repository named after a sub-resource is still reachable.
#[tokio::test]
async fn a_repository_named_like_a_sub_resource_is_reachable() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "reserved").await;

    for name in RESERVED {
        create_repo(&app, &user.token, name).await;
    }

    for name in RESERVED {
        let uri = format!("/api/repos/reserved/{name}");
        let (status, body) = request(&app, Method::GET, &uri, None, Some(&user.token)).await;
        assert_eq!(status, StatusCode::OK, "GET {uri}: {body}");
        assert_eq!(
            body.get("name").and_then(|v| v.as_str()),
            Some(*name),
            "GET {uri} returned another repository: {body}"
        );
        assert_eq!(
            body.get("full_name").and_then(|v| v.as_str()),
            Some(format!("reserved/{name}").as_str()),
            "GET {uri} returned another repository: {body}"
        );
    }
}

/// The sub-resources of an ordinary repository keep working.
#[tokio::test]
async fn sub_resources_of_an_ordinary_repository_still_resolve() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "ordinary").await;
    create_repo(&app, &user.token, "widgets").await;

    for suffix in ["/tree", "/dir", "/commits", "/branches", "/tags", "/hooks"] {
        let uri = format!("/api/repos/ordinary/widgets{suffix}");
        let (status, body) = request(&app, Method::GET, &uri, None, Some(&user.token)).await;
        assert_eq!(status, StatusCode::OK, "GET {uri}: {body}");
    }

    // `/file` needs a `path`; the point is that the request reached the
    // file handler rather than being mistaken for a repository path.
    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/ordinary/widgets/file",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "GET .../file: {body}");
    assert!(
        !body.to_string().contains("invalid repository path"),
        "GET .../file was routed as a repository path: {body}"
    );
}

/// A repository named after a sub-resource still serves that
/// sub-resource — the name only shadows the *repository* interpretation
/// of a path, not the sub-resource of the same repository.
#[tokio::test]
async fn a_repository_named_like_a_sub_resource_still_serves_it() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "shadowed").await;
    create_repo(&app, &user.token, "dir").await;

    // The tree of the repository named `dir`.
    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/shadowed/dir/tree",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "tree of the repo named dir: {body}");

    // The directory listing of the same repository.
    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/shadowed/dir/dir",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "directory listing of the repo named dir: {body}"
    );
}

/// A repository named after a sub-resource in a group namespace is
/// reachable too — the group path is not a repository, so the marker
/// interpretation must not win.
#[tokio::test]
async fn a_group_repository_named_like_a_sub_resource_is_reachable() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "groupowner").await;

    let (status, body) = request(
        &app,
        Method::POST,
        "/api/groups",
        Some(serde_json::json!({"name": "acme", "description": ""})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create group: {body}");

    let (status, body) = request(
        &app,
        Method::POST,
        "/api/groups/acme/repos",
        Some(serde_json::json!({
            "name": "tree",
            "description": "",
            "default_branch": "main",
            "is_private": false,
        })),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create group repo: {body}");

    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/acme/tree",
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "GET group repo named tree: {body}");
    assert_eq!(
        body.get("full_name").and_then(|v| v.as_str()),
        Some("acme/tree"),
        "GET group repo named tree: {body}"
    );

    // The tree of that repository still resolves.
    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/acme/tree/tree",
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "tree of the group repo: {body}");
}

/// A repository named after a sub-resource can be deleted — the delete
/// dispatcher must not read the trailing segment as a sub-resource.
#[tokio::test]
async fn a_repository_named_like_a_sub_resource_can_be_deleted() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "deletable").await;
    create_repo(&app, &user.token, "star").await;
    create_repo(&app, &user.token, "hooks").await;

    for name in ["star", "hooks"] {
        let uri = format!("/api/repos/deletable/{name}");
        let (status, body) = request(&app, Method::DELETE, &uri, None, Some(&user.token)).await;
        assert_eq!(status, StatusCode::OK, "DELETE {uri}: {body}");
    }

    // The repositories are gone.
    for name in ["star", "hooks"] {
        let uri = format!("/api/repos/deletable/{name}");
        let (status, _) = request(&app, Method::GET, &uri, None, Some(&user.token)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {uri} after delete");
    }
}
