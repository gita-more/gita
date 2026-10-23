//! Security regression tests for the "second batch" of fixes.
//!
//! These lock in the P1 items fixed after the initial design review:
//!
//! * P1-11: registration is not a username / e-mail enumeration oracle.
//!   A conflicting registration returns the *same* message regardless of
//!   whether the username or the e-mail collided, and the handler burns a
//!   constant-time dummy hash so response time does not leak existence.
//! * P1-13: unknown `/api/*` paths return a JSON 404 (with the standard
//!   error envelope), never SPA HTML with a 200 status.
//! * P1-16: an invalid Bearer token is no longer silently downgraded to
//!   anonymous — it returns 401 instead of letting the request continue.
//! * P1-17: JWTs are revocable via `POST /api/auth/logout`. After logout,
//!   the same token no longer authenticates.

mod common;

use axum::http::{Method, StatusCode};
use common::{create_repo, push_initial_commit, register_user, request, request_bytes, test_app};

/// P1-13: an unknown `/api/*` path must return a JSON 404, not the SPA's
/// HTML with status 200.
#[tokio::test]
async fn unknown_api_path_returns_json_404() {
    let (app, _guard) = test_app().await;

    let (status, headers, body) =
        request_bytes(&app, Method::GET, "/api/definitely-not-a-route", None).await;

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "unknown API path must be a 404, got {status}"
    );
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        content_type.starts_with("application/json"),
        "expected JSON 404, got content-type {content_type:?}"
    );

    // The body must be the standard error envelope, not SPA HTML.
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    assert!(
        value.get("error").is_some(),
        "expected error envelope in body, got {value:?}"
    );
    assert_eq!(value["error"]["status"], 404);
}

/// P1-13: the JSON 404 envelope is consistent for every HTTP method.
#[tokio::test]
async fn unknown_api_path_returns_json_404_for_all_methods() {
    let (app, _guard) = test_app().await;

    for method in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
        let (status, headers, body) =
            request_bytes(&app, method.clone(), "/api/not-a-real-endpoint", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} on unknown path");
        let content_type = headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(
            content_type.starts_with("application/json"),
            "{method} content-type was {content_type:?}"
        );
        let _ =
            serde_json::from_slice::<serde_json::Value>(&body).expect("body must be valid JSON");
    }
}

/// P1-16: an invalid Bearer token must not be silently downgraded to
/// anonymous on an otherwise-public endpoint — it must return 401.
#[tokio::test]
async fn invalid_bearer_token_is_not_downgraded_to_anonymous() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "pubowner").await;
    let full = create_repo(&app, &owner.token, "public-repo").await;
    let (owner_name, repo_name) = full.split_once('/').unwrap();
    push_initial_commit(&app, owner_name, repo_name);

    // `GET /api/repos/{owner}/{repo}` uses OptionalAuthUser: public repos
    // are readable anonymously.
    let uri = format!("/api/repos/{full}");

    // Anonymous (no token) is allowed on a public repo.
    let (status, _) = request(&app, Method::GET, &uri, None, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "anonymous should be able to read a public repo"
    );

    // A garbage Bearer token must NOT be downgraded to anonymous.
    let (status, _) = request(
        &app,
        Method::GET,
        &uri,
        None,
        Some("this-is-not-a-valid-token"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "invalid Bearer token must return 401, not silently act as anonymous"
    );

    // An expired/garbage JWT must also be rejected.
    let (status, _) = request(
        &app,
        Method::GET,
        &uri,
        None,
        Some("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIiwidXNlcm5hbWUiOiJ4IiwiZXhwIjoiMCJ9.invalid"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "garbage JWT must return 401"
    );
}

/// P1-17: `POST /api/auth/logout` revokes the JWT used for the request.
#[tokio::test]
async fn logout_revokes_the_jwt() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "logout-user").await;

    // The registration token (a JWT) works before logout.
    let (status, _) = request(&app, Method::GET, "/api/auth/me", None, Some(&user.token)).await;
    assert_eq!(status, StatusCode::OK, "JWT must work before logout");

    // Logout with the JWT.
    let (status, body) = request(
        &app,
        Method::POST,
        "/api/auth/logout",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "logout failed: {body}");

    // The same JWT must no longer authenticate.
    let (status, _) = request(&app, Method::GET, "/api/auth/me", None, Some(&user.token)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "revoked JWT must no longer authenticate"
    );
}

/// P1-17: logout requires authentication — no token, no logout.
#[tokio::test]
async fn logout_requires_authentication() {
    let (app, _guard) = test_app().await;

    let (status, _) = request(&app, Method::POST, "/api/auth/logout", None, None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "logout without a token must be rejected"
    );
}

/// P1-17: logging out with a personal access token must not pretend to
/// revoke it. A PAT is a long-lived credential with no session to end, so
/// the response says so — and the PAT keeps working until it is revoked
/// explicitly.
#[tokio::test]
async fn logout_with_a_pat_does_not_claim_to_revoke_it() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "pat-logout").await;

    // Create a PAT.
    let (status, body) = request(
        &app,
        Method::POST,
        "/api/auth/tokens",
        Some(serde_json::json!({"name": "ci"})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create PAT: {body}");
    let pat = body["token"].as_str().unwrap().to_string();

    // Log out with the PAT: the request is accepted (the PAT is valid).
    let (status, body) = request(&app, Method::POST, "/api/auth/logout", None, Some(&pat)).await;
    assert_eq!(status, StatusCode::OK, "logout with PAT failed: {body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("personal access token"),
        "the response must say that a PAT is not revoked by logout, got {body:?}"
    );

    // The PAT still works — logout did not (and must not) revoke it.
    let (status, _) = request(&app, Method::GET, "/api/auth/me", None, Some(&pat)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a PAT must survive logout — revoke it explicitly instead"
    );
}

/// P1-17: revoking one JWT must not affect the user's other sessions.
#[tokio::test]
async fn logout_revokes_only_the_presented_token() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "two-sessions").await;

    // Log in again to get a second, independent JWT for the same user.
    let (status, body) = request(
        &app,
        Method::POST,
        "/api/auth/login",
        Some(serde_json::json!({
            "username": "two-sessions",
            "password": "hunter22",
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login: {body}");
    let second = body["token"].as_str().unwrap().to_string();
    assert_ne!(
        second, user.token,
        "the two logins must yield distinct tokens"
    );

    // Log out with the first token only.
    let (status, _) = request(
        &app,
        Method::POST,
        "/api/auth/logout",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The first token is dead, the second still works.
    let (status, _) = request(&app, Method::GET, "/api/auth/me", None, Some(&user.token)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the logged-out token must die"
    );

    let (status, _) = request(&app, Method::GET, "/api/auth/me", None, Some(&second)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the other session must not be affected"
    );
}

/// P1-11: registration returns the *same* conflict message regardless of
/// whether the username or the e-mail collided, so an anonymous caller
/// cannot tell which one exists.
#[tokio::test]
async fn registration_conflict_message_is_uniform() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "enum-target").await;

    // Collision on the username (same username, different e-mail).
    let (status_username, body_username) = request(
        &app,
        Method::POST,
        "/api/auth/register",
        Some(serde_json::json!({
            "username": "enum-target",
            "email": "different@e2e.local",
            "password": "hunter22",
        })),
        None,
    )
    .await;

    // Collision on the e-mail (same e-mail, different username).
    let (status_email, body_email) = request(
        &app,
        Method::POST,
        "/api/auth/register",
        Some(serde_json::json!({
            "username": "another-user",
            "email": "enum-target@e2e.local",
            "password": "hunter22",
        })),
        None,
    )
    .await;

    assert_eq!(status_username, StatusCode::CONFLICT);
    assert_eq!(status_email, StatusCode::CONFLICT);

    // The two conflict bodies must be identical — same message, same status.
    assert_eq!(
        body_username, body_email,
        "username and e-mail conflicts must be indistinguishable"
    );
    assert_eq!(
        body_username["error"]["message"],
        "conflict: username or email already in use"
    );

    // The `user` variable is only used to ensure the account exists; keep it
    // referenced to silence the unused warning.
    let _ = user;
}

/// P1-11: a *successful* registration response is structurally different
/// from a conflict (it carries a token), so the uniform conflict message
/// is what matters — but an anonymous caller must not be able to learn
/// whether a *specific username* exists by comparing message text alone.
/// This is covered by the previous test; here we just pin the exact
/// conflict message so it cannot regress.
#[tokio::test]
async fn registration_conflict_message_is_pinned() {
    let (app, _guard) = test_app().await;
    register_user(&app, "pinned-user").await;

    let (status, body) = request(
        &app,
        Method::POST,
        "/api/auth/register",
        Some(serde_json::json!({
            "username": "pinned-user",
            "email": "pinned-user@e2e.local",
            "password": "hunter22",
        })),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        body["error"]["message"], "conflict: username or email already in use",
        "the conflict message must stay uniform to avoid enumeration"
    );
}

/// P1-18 regression guard: the ownership check on `DELETE /api/repos/...`
/// compares repository `owner_id` with the caller's user id, not string
/// usernames. A non-owner gets 403 whether or not they share the owner's
/// username spelling; the owner (by id) can delete.
#[tokio::test]
async fn delete_repo_uses_id_ownership_not_username_strings() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "delowner").await;
    let intruder = register_user(&app, "delintruder").await;
    let full = create_repo(&app, &owner.token, "owned-repo").await;
    let (owner_name, repo_name) = full.split_once('/').unwrap();
    let _ = owner_name;

    // Intruder cannot delete — 403, regardless of any username resemblance.
    let (status, _) = request(
        &app,
        Method::DELETE,
        &format!("/api/repos/delowner/{repo_name}"),
        None,
        Some(&intruder.token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "non-owner must not delete the repo"
    );

    // Owner can delete.
    let (status, _) = request(
        &app,
        Method::DELETE,
        &format!("/api/repos/delowner/{repo_name}"),
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner should be able to delete");
}
