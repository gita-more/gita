mod common;

use common::{register_user, request};

#[tokio::test]
async fn group_member_management() {
    let (app, _guard) = common::test_app().await;
    let owner = register_user(&app, "grp-owner").await;
    let member = register_user(&app, "grp-member").await;

    // Owner creates a group.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups",
        Some(serde_json::json!({"name": "acme", "description": ""})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create group: {body}");

    // Add member.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups/acme/members",
        Some(serde_json::json!({"username": "grp-member", "role": "developer"})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "add member: {body}");
    assert_eq!(body["role"], "developer");

    // List members.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/groups/acme/members",
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let members = body["members"].as_array().unwrap();
    assert!(members.iter().any(|m| m["username"] == "grp-member"));
    assert!(members.iter().any(|m| m["username"] == "grp-owner")); // owner is auto-added

    // Non-owner cannot add a member.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups/acme/members",
        Some(serde_json::json!({"username": "grp-owner"})),
        Some(&member.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::FORBIDDEN);

    // Update role.
    let (status, _) = request(
        &app,
        axum::http::Method::PATCH,
        "/api/groups/acme/members/grp-member",
        Some(serde_json::json!({"role": "maintainer"})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Invalid role -> 400.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups/acme/members",
        Some(serde_json::json!({"username": "grp-member", "role": "admin"})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);

    // Remove member.
    let (status, _) = request(
        &app,
        axum::http::Method::DELETE,
        "/api/groups/acme/members/grp-member",
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Removing twice -> 404.
    let (status, _) = request(
        &app,
        axum::http::Method::DELETE,
        "/api/groups/acme/members/grp-member",
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
}

/// The `owner` role is reserved for the group creator. `create_group`
/// records the creator in `group_members` with role `owner`, but the API
/// must not hand that role to anyone else — a second "owner" would look
/// authoritative while `require_group_owner` still only honours
/// `groups.owner_id`.
#[tokio::test]
async fn group_owner_role_cannot_be_assigned_to_others() {
    let (app, _guard) = common::test_app().await;
    let owner = register_user(&app, "role-owner").await;
    let member = register_user(&app, "role-member").await;

    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups",
        Some(serde_json::json!({"name": "roles", "description": ""})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Adding the member with a normal role works.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups/roles/members",
        Some(serde_json::json!({"username": "role-member", "role": "developer"})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "add member: {body}");
    assert_eq!(body["role"], "developer");

    // Assigning `owner` to someone else is rejected.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups/roles/members",
        Some(serde_json::json!({"username": "role-member", "role": "owner"})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::BAD_REQUEST,
        "assigning owner must be rejected: {body}"
    );

    // Same for the update (PATCH) path.
    let (status, body) = request(
        &app,
        axum::http::Method::PATCH,
        "/api/groups/roles/members/role-member",
        Some(serde_json::json!({"role": "owner"})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::BAD_REQUEST,
        "promoting to owner must be rejected: {body}"
    );

    // The member kept the role they were given.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/groups/roles/members",
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let members = body["members"].as_array().unwrap();
    let m = members
        .iter()
        .find(|m| m["username"] == "role-member")
        .expect("member row");
    assert_eq!(m["role"], "developer", "role must not have changed");

    // The creator still carries role `owner` — the row is created by
    // `create_group`, not through the API, so rejecting the role above
    // must not affect it.
    let owner_row = members
        .iter()
        .find(|m| m["username"] == "role-owner")
        .expect("owner row");
    assert_eq!(
        owner_row["role"], "owner",
        "the group creator keeps the owner role"
    );
}

/// Group membership is a private listing: without authentication a caller
/// could enumerate who belongs to every group in the instance.
#[tokio::test]
async fn group_members_require_owner_or_member() {
    let (app, _guard) = common::test_app().await;
    let owner = register_user(&app, "mem-owner").await;
    let member = register_user(&app, "mem-member").await;
    let outsider = register_user(&app, "mem-outsider").await;

    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups",
        Some(serde_json::json!({"name": "closed", "description": ""})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups/closed/members",
        Some(serde_json::json!({"username": "mem-member", "role": "member"})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Anonymous -> 401 (the route requires a bearer token).
    let (status, _) = request(
        &app,
        axum::http::Method::GET,
        "/api/groups/closed/members",
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNAUTHORIZED,
        "anonymous member listing must be rejected"
    );

    // Authenticated but not a member -> 403.
    let (status, _) = request(
        &app,
        axum::http::Method::GET,
        "/api/groups/closed/members",
        None,
        Some(&outsider.token),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::FORBIDDEN,
        "a non-member must not list the membership"
    );

    // The owner can list.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/groups/closed/members",
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body["members"].as_array().unwrap().len() >= 2);

    // So can a member.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/groups/closed/members",
        None,
        Some(&member.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body["members"].as_array().unwrap().len() >= 2);
}

/// `GET /api/groups/{group}` stays readable by anonymous callers (it only
/// exposes the group's *public* repositories), but a private repository
/// must never appear in that listing.
#[tokio::test]
async fn group_detail_hides_private_repositories_from_anonymous_callers() {
    let (app, _guard) = common::test_app().await;
    let owner = register_user(&app, "vis-owner").await;

    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups",
        Some(serde_json::json!({"name": "mixed", "description": ""})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // A public repository in the group.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups/mixed/repos",
        Some(serde_json::json!({"name": "public-repo", "is_private": false})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // And a private one.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups/mixed/repos",
        Some(serde_json::json!({"name": "secret-repo", "is_private": true})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "create private repo: {body}"
    );

    // Anonymous: the public repo is listed, the private one is not.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/groups/mixed",
        None,
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let repos = body["repos"].as_array().unwrap();
    let names: Vec<&str> = repos.iter().filter_map(|r| r["name"].as_str()).collect();
    assert!(
        names.contains(&"public-repo"),
        "public repo must be listed, got {names:?}"
    );
    assert!(
        !names.contains(&"secret-repo"),
        "private repo leaked to an anonymous caller: {names:?}"
    );

    // The group's own owner still sees both.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/groups/mixed",
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let repos = body["repos"].as_array().unwrap();
    let names: Vec<&str> = repos.iter().filter_map(|r| r["name"].as_str()).collect();
    assert!(
        names.contains(&"secret-repo"),
        "the owner must still see the private repo, got {names:?}"
    );
}
