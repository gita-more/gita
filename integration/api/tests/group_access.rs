//! Group repository access control.
//!
//! A repository in a group namespace used to be reachable only by the
//! group's creator: every access check keyed on `owner_id`, which for a
//! group repository is the id of the user who created the *group*. A
//! member added to the group could not read the group's private
//! repositories, could not file an issue in them, and could not push to
//! them — even when the repository was public and even when the group's
//! own role field said they were a developer.
//!
//! The rule now lives in one place (`gita_core::access`) and the group's
//! roles take part in it. These tests pin the resulting behaviour.
//!
//! The roles, from least to most privileged: `guest`, `member`,
//! `developer`, `maintainer`, and `owner` — the last one being reserved
//! for the group's creator.

mod common;

use axum::http::{Method, StatusCode};
use common::{register_user, request, test_app};

/// Create a group owned by `owner` and return its name.
async fn create_group(app: &gita_server::App, _owner: &str, token: &str, name: &str) {
    let (status, body) = request(
        app,
        Method::POST,
        "/api/groups",
        Some(serde_json::json!({"name": name, "description": ""})),
        Some(token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create group {name}: {body}");
}

/// Add `username` to the group with the given role.
async fn add_member(
    app: &gita_server::App,
    group: &str,
    owner_token: &str,
    username: &str,
    role: &str,
) {
    let (status, body) = request(
        app,
        Method::POST,
        &format!("/api/groups/{group}/members"),
        Some(serde_json::json!({"username": username, "role": role})),
        Some(owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "add {username} as {role}: {body}");
}

/// Create a repository inside the group.
async fn create_group_repo(
    app: &gita_server::App,
    group: &str,
    token: &str,
    name: &str,
    is_private: bool,
) {
    let (status, body) = request(
        app,
        Method::POST,
        &format!("/api/groups/{group}/repos"),
        Some(serde_json::json!({
            "name": name,
            "description": "",
            "default_branch": "main",
            "is_private": is_private,
        })),
        Some(token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "create group repo {group}/{name}: {body}"
    );
}

/// A member of a group can file an issue in the group's repository.
///
/// This is the behaviour that was missing: a write path required
/// `user.id == repository.owner_id`, and for a group repository that is
/// the group's creator, not the member.
#[tokio::test]
async fn a_group_member_can_file_an_issue_in_a_group_repository() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "ga-owner").await;
    let member = register_user(&app, "ga-member").await;

    create_group(&app, "acme", &owner.token, "acme").await;
    add_member(&app, "acme", &owner.token, "ga-member", "member").await;
    create_group_repo(&app, "acme", &owner.token, "widgets", false).await;

    let (status, body) = request(
        &app,
        Method::POST,
        "/api/repos/acme/widgets/issues",
        Some(serde_json::json!({"title": "from a member", "body": "hello"})),
        Some(&member.token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a group member must be able to file an issue: {body}"
    );
    assert!(
        body["id"].as_i64().unwrap() > 0,
        "issue must have a real id"
    );
}

/// A member can read a *private* group repository; a non-member cannot.
#[tokio::test]
async fn a_private_group_repository_is_visible_to_its_members_only() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "gp-owner").await;
    let member = register_user(&app, "gp-member").await;
    let outsider = register_user(&app, "gp-outsider").await;

    create_group(&app, "secret", &owner.token, "secret").await;
    add_member(&app, "secret", &owner.token, "gp-member", "member").await;
    create_group_repo(&app, "secret", &owner.token, "vault", true).await;

    // A member sees it.
    let (status, _) = request(
        &app,
        Method::GET,
        "/api/repos/secret/vault",
        None,
        Some(&member.token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a group member must be able to read the private group repo"
    );

    // An outsider must not — and must not learn that it exists.
    let (status, _) = request(
        &app,
        Method::GET,
        "/api/repos/secret/vault",
        None,
        Some(&outsider.token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a non-member must not see a private group repo"
    );
}

/// A guest sees the group's public repositories and nothing else.
#[tokio::test]
async fn a_guest_is_limited_to_the_public_group_repositories() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "gg-owner").await;
    let guest = register_user(&app, "gg-guest").await;

    create_group(&app, "mixed", &owner.token, "mixed").await;
    add_member(&app, "mixed", &owner.token, "gg-guest", "guest").await;
    create_group_repo(&app, "mixed", &owner.token, "open", false).await;
    create_group_repo(&app, "mixed", &owner.token, "closed", true).await;

    // The public one is readable.
    let (status, _) = request(
        &app,
        Method::GET,
        "/api/repos/mixed/open",
        None,
        Some(&guest.token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a guest may read a public group repo"
    );

    // The private one is not.
    let (status, _) = request(
        &app,
        Method::GET,
        "/api/repos/mixed/closed",
        None,
        Some(&guest.token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a guest must not read a private group repo"
    );

    // And a guest may not file issues.
    let (status, _) = request(
        &app,
        Method::POST,
        "/api/repos/mixed/open/issues",
        Some(serde_json::json!({"title": "nope", "body": ""})),
        Some(&guest.token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a guest must not write to a group repo"
    );
}

/// Only the group owner may administer the group's repository, no matter
/// how privileged the caller's role is.
#[tokio::test]
async fn only_the_group_owner_may_administer_a_group_repository() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "gm-owner").await;
    let maintainer = register_user(&app, "gm-maintainer").await;

    create_group(&app, "ops", &owner.token, "ops").await;
    add_member(&app, "ops", &owner.token, "gm-maintainer", "maintainer").await;
    create_group_repo(&app, "ops", &owner.token, "infra", false).await;

    // A maintainer may not change the repository's settings.
    let (status, _) = request(
        &app,
        Method::PATCH,
        "/api/repos/ops/infra",
        Some(serde_json::json!({"is_private": true})),
        Some(&maintainer.token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a maintainer must not change the repository's settings"
    );

    // The group's owner may.
    let (status, body) = request(
        &app,
        Method::PATCH,
        "/api/repos/ops/infra",
        Some(serde_json::json!({"is_private": true})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the group owner must be able to administer the repository: {body}"
    );
}

/// A member of a group may not touch a repository that is not the group's.
#[tokio::test]
async fn group_membership_does_not_grant_access_to_other_repositories() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "gx-owner").await;
    let member = register_user(&app, "gx-member").await;

    create_group(&app, "team", &owner.token, "team").await;
    add_member(&app, "team", &owner.token, "gx-member", "developer").await;

    // A *private* personal repository of the group's owner.
    let (status, body) = request(
        &app,
        Method::POST,
        "/api/repos",
        Some(serde_json::json!({
            "name": "private-notes",
            "description": "",
            "default_branch": "main",
            "is_private": true,
        })),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create private repo: {body}");

    let (status, _) = request(
        &app,
        Method::GET,
        "/api/repos/gx-owner/private-notes",
        None,
        Some(&member.token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "membership of one group must not unlock another repository"
    );
}
