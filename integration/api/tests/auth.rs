mod common;

use common::{register_user, request};

#[tokio::test]
async fn register_then_login_then_me() {
    let (app, _guard) = common::test_app().await;

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
    assert_eq!(body["user"]["locale"], "zh", "default locale should be zh");

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
    assert_eq!(body["locale"], "zh");

    // PATCH locale -> en, then me reflects it
    let (status, body) = request(
        &app,
        axum::http::Method::PATCH,
        "/api/auth/me",
        Some(serde_json::json!({ "locale": "en" })),
        Some(&token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "patch locale: {body}");
    assert_eq!(body["locale"], "en");

    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/auth/me",
        None,
        Some(&token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "me after patch: {body}");
    assert_eq!(body["locale"], "en");
}

#[tokio::test]
async fn register_validation_errors() {
    let (app, _guard) = common::test_app().await;

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
    let (app, _guard) = common::test_app().await;
    let _ = register_user(&app, "carol").await;

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

#[tokio::test]
async fn system_init_creates_first_admin_and_only_runs_once() {
    let (app, _guard) = common::test_app().await;

    // Before any account exists, the system reports uninitialized.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/auth/initialized",
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "initialized probe: {body}"
    );
    assert_eq!(body["initialized"], false);

    // Create the first admin.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/init",
        Some(serde_json::json!({
            "username": "root",
            "email": "root@example.com",
            "password": "hunter22",
        })),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "init: {body}");
    assert_eq!(body["user"]["username"], "root");
    assert!(
        body["token"].as_str().unwrap().len() >= 32,
        "init should mint a JWT"
    );
    let token = body["token"].as_str().unwrap().to_string();

    // The created account is authenticated and usable.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/auth/me",
        None,
        Some(&token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "me as root: {body}");
    assert_eq!(body["username"], "root");

    // After initialization the system reports initialized.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/auth/initialized",
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "initialized probe: {body}"
    );
    assert_eq!(body["initialized"], true);

    // A second init attempt is rejected.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/init",
        Some(serde_json::json!({
            "username": "root2",
            "email": "root2@example.com",
            "password": "hunter22",
        })),
        None,
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::BAD_REQUEST,
        "second init: {body}"
    );
}

#[tokio::test]
async fn system_init_is_rejected_once_any_user_exists() {
    let (app, _guard) = common::test_app().await;
    let _ = register_user(&app, "earlybird").await;

    // The system reports initialized once a normal account exists.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/auth/initialized",
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "initialized probe: {body}"
    );
    assert_eq!(body["initialized"], true);

    // init cannot be used to seize the system after an account exists.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/init",
        Some(serde_json::json!({
            "username": "root",
            "email": "root@example.com",
            "password": "hunter22",
        })),
        None,
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::BAD_REQUEST,
        "init after user exists: {body}"
    );
}

#[tokio::test]
async fn access_tokens_create_and_list() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "token-user").await;

    // create a token
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/tokens",
        Some(serde_json::json!({"name": "ci"})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create token: {body}");
    let new_token = body["token"].as_str().unwrap();
    assert!(new_token.len() >= 32, "token should be reasonably long");

    // list tokens
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/auth/tokens",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let arr = body.as_array().unwrap();
    assert!(arr.iter().any(|t| t["name"] == "ci"));
}

#[tokio::test]
async fn personal_access_token_revocation() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "pat-user").await;

    // Create two tokens.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/tokens",
        Some(serde_json::json!({"name": "ci"})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let token_id = body["id"].as_i64().unwrap();
    let raw_token = body["token"].as_str().unwrap().to_string();

    // Token works for auth.
    let (status, _) = request(
        &app,
        axum::http::Method::GET,
        "/api/auth/me",
        None,
        Some(&raw_token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Delete the token.
    let (status, _) = request(
        &app,
        axum::http::Method::DELETE,
        &format!("/api/auth/tokens/{token_id}"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Token no longer authenticates.
    let (status, _) = request(
        &app,
        axum::http::Method::GET,
        "/api/auth/me",
        None,
        Some(&raw_token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);

    // Deleting a non-existent token -> 404.
    let (status, _) = request(
        &app,
        axum::http::Method::DELETE,
        &format!("/api/auth/tokens/{token_id}"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn change_password_and_email() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "pw-user").await;

    // Change password — wrong current password.
    let (status, _) = request(
        &app,
        axum::http::Method::PATCH,
        "/api/auth/me",
        Some(serde_json::json!({
            "current_password": "wrong",
            "new_password": "newpass99",
        })),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);

    // Change password — missing current.
    let (status, _) = request(
        &app,
        axum::http::Method::PATCH,
        "/api/auth/me",
        Some(serde_json::json!({"new_password": "newpass99"})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);

    // Change password — successful.
    let (status, _) = request(
        &app,
        axum::http::Method::PATCH,
        "/api/auth/me",
        Some(serde_json::json!({
            "current_password": "hunter22",
            "new_password": "newpass99",
        })),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Old password no longer works.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/login",
        Some(serde_json::json!({"username": "pw-user", "password": "hunter22"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);

    // New password works.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/login",
        Some(serde_json::json!({"username": "pw-user", "password": "newpass99"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Change email — successful.
    let new_token = {
        let (status, body) = request(
            &app,
            axum::http::Method::POST,
            "/api/auth/login",
            Some(serde_json::json!({"username": "pw-user", "password": "newpass99"})),
            None,
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        body["token"].as_str().unwrap().to_string()
    };

    let (status, body) = request(
        &app,
        axum::http::Method::PATCH,
        "/api/auth/me",
        Some(serde_json::json!({
            "current_password": "newpass99",
            "email": "pw-user-new@example.com",
        })),
        Some(&new_token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "patch email: {body}");
    assert_eq!(body["email"], "pw-user-new@example.com");

    // Duplicate email -> 409.
    let other = register_user(&app, "pw-other").await;
    let (status, _) = request(
        &app,
        axum::http::Method::PATCH,
        "/api/auth/me",
        Some(serde_json::json!({
            "current_password": "newpass99",
            "email": "pw-other@e2e.local",
        })),
        Some(&new_token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CONFLICT);

    // Short new password -> 400.
    let (status, _) = request(
        &app,
        axum::http::Method::PATCH,
        "/api/auth/me",
        Some(serde_json::json!({
            "current_password": "newpass99",
            "new_password": "123",
        })),
        Some(&new_token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    let _ = other;
}

#[tokio::test]
async fn register_then_login_chain_succeeds_for_second_user() {
    // Verifies a fresh user can complete both flows in one test without
    // hidden state from a previous test (token issuing/signing).
    let (app, _guard) = common::test_app().await;
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/register",
        Some(serde_json::json!({
            "username": "chain-user",
            "email": "chain@example.com",
            "password": "hunter22",
        })),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "register: {body}");
    let token = body["token"].as_str().unwrap().to_string();

    // login with same creds — different JWT shape (different iat/exp)
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/login",
        Some(serde_json::json!({
            "username": "chain-user",
            "password": "hunter22",
        })),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let new_token = body["token"].as_str().unwrap();
    // Note: two logins within the same second may produce identical JWTs
    // because claims are second-precision; we only assert validity here.

    // both tokens are valid against /me
    for t in [token, new_token.to_string()] {
        let (status, body) = request(
            &app,
            axum::http::Method::GET,
            "/api/auth/me",
            None,
            Some(&t),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(body["username"], "chain-user");
    }
}

/// A username becomes a directory name under the repositories root
/// (`<data_dir>/repos/<username>/`), so it must not be able to escape it.
///
/// The registration endpoint checked only the length, so `username = "../.."`
/// was accepted and a subsequent repository creation ran `init_bare` on a
/// path outside the repositories root.
#[tokio::test]
async fn register_rejects_usernames_that_could_escape_the_repos_root() {
    let (app, _guard) = common::test_app().await;

    for username in ["../..", "..", ".", "a/b", "a\\b", "a b"] {
        let (status, body) = common::request(
            &app,
            axum::http::Method::POST,
            "/api/auth/register",
            Some(serde_json::json!({
                "username": username,
                "email": format!("{username}@example.com"),
                "password": "hunter22",
            })),
            None,
        )
        .await;
        assert_eq!(
            status,
            axum::http::StatusCode::BAD_REQUEST,
            "username {username:?} should be rejected, got {status} {body}"
        );
    }
}
