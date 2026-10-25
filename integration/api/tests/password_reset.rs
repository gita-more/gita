mod common;

use common::{register_user, request};

/// By default the reset token is *not* returned in the response: the
/// response is identical for existing and non-existent accounts, so
/// handing back the token would let an anonymous caller enumerate
/// accounts and take them over.
#[tokio::test]
async fn password_reset_token_is_withheld_by_default() {
    let (app, _guard) = common::test_app().await;
    register_user(&app, "withheld-user").await;

    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/request",
        Some(serde_json::json!({"identifier": "withheld-user"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        body.get("reset_token").is_none(),
        "the token must not be in the response by default: {body}"
    );
    assert!(
        body.get("message").is_some(),
        "the uniform message must still be present: {body}"
    );

    // The same for an account that does not exist — the two responses
    // must be indistinguishable.
    let (status, missing) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/request",
        Some(serde_json::json!({"identifier": "no-such-account"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(
        missing, body,
        "an existing and a non-existent account must produce the same response"
    );
}

/// With `allow_insecure_password_reset` on, the response carries the token
/// so an operator can complete the flow without a mail transport. The
/// token must actually work.
#[tokio::test]
async fn password_reset_token_is_returned_when_enabled() {
    let (app, _guard) = common::test_app_with_insecure_password_reset().await;
    register_user(&app, "insecure-user").await;

    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/request",
        Some(serde_json::json!({"identifier": "insecure-user"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let token = body["reset_token"]
        .as_str()
        .expect("reset_token must be present when the flag is on")
        .to_string();
    assert!(!token.is_empty(), "reset_token must not be empty");

    // The token completes the reset.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/confirm",
        Some(serde_json::json!({"token": token, "new_password": "brandnew1"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "confirm: {body}");

    // And the new password works.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/login",
        Some(serde_json::json!({"username": "insecure-user", "password": "brandnew1"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
}

/// A non-existent account must not produce a `reset_token` even when the
/// flag is on — otherwise the field's presence enumerates accounts.
#[tokio::test]
async fn password_reset_token_is_absent_for_unknown_accounts() {
    let (app, _guard) = common::test_app_with_insecure_password_reset().await;

    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/request",
        Some(serde_json::json!({"identifier": "ghost-account"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        body.get("reset_token").is_none(),
        "no token may be issued for an unknown account: {body}"
    );
}

/// Requesting a second token must not invalidate the first one — and the
/// stale-token cleanup must not remove tokens that are still valid.
#[tokio::test]
async fn issuing_a_second_token_keeps_the_first_valid() {
    let (app, _guard) = common::test_app_with_insecure_password_reset().await;
    register_user(&app, "two-token-user").await;

    let first = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/request",
        Some(serde_json::json!({"identifier": "two-token-user"})),
        None,
    )
    .await
    .1["reset_token"]
        .as_str()
        .unwrap()
        .to_string();
    let second = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/request",
        Some(serde_json::json!({"identifier": "two-token-user"})),
        None,
    )
    .await
    .1["reset_token"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(first, second, "each request must issue a distinct token");

    // The first token is still redeemable.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/confirm",
        Some(serde_json::json!({"token": first, "new_password": "firstpass1"})),
        None,
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "a still-valid token must survive a second request: {body}"
    );

    // The second one is too — until it is used.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/confirm",
        Some(serde_json::json!({"token": second, "new_password": "secondpass1"})),
        None,
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "the second token must still be valid: {body}"
    );
}

/// Used tokens are cleaned up: requesting a new token after one was
/// redeemed must not leave the spent row behind.
#[tokio::test]
async fn spent_reset_tokens_are_purged() {
    let (app, guard) = common::test_app_with_insecure_password_reset().await;
    let schema = guard.schema.clone();
    register_user(&app, "purge-user").await;

    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/request",
        Some(serde_json::json!({"identifier": "purge-user"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let token = body["reset_token"].as_str().unwrap().to_string();

    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/confirm",
        Some(serde_json::json!({"token": token, "new_password": "purgepass1"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Requesting another token triggers the cleanup of the spent row.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/request",
        Some(serde_json::json!({"identifier": "purge-user"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    let pool = sqlx::PgPool::connect(&common::test_db_url()).await.unwrap();
    let count: (i64,) = sqlx::query_as(&format!(
        "SELECT count(*) FROM \"{schema}\".password_reset_tokens WHERE used = TRUE"
    ))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count.0, 0,
        "the spent token must have been purged, found {} row(s)",
        count.0
    );
}

/// A reset token is single-use: once it has been redeemed it must not work
/// a second time.
#[tokio::test]
async fn a_reset_token_cannot_be_redeemed_twice() {
    let (app, _guard) = common::test_app_with_insecure_password_reset().await;
    register_user(&app, "single-use-user").await;

    let (_, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/request",
        Some(serde_json::json!({"identifier": "single-use-user"})),
        None,
    )
    .await;
    let token = body["reset_token"].as_str().unwrap().to_string();

    // First redemption works.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/confirm",
        Some(serde_json::json!({"token": token, "new_password": "firstpass1"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // The second must be refused — otherwise the token is a bearer
    // credential that can be replayed to take the account over again.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/confirm",
        Some(serde_json::json!({"token": token, "new_password": "secondpass1"})),
        None,
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::NOT_FOUND,
        "a spent token must not be redeemable again: {body}"
    );

    // The first password still stands: the replayed attempt did not
    // overwrite it.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/login",
        Some(serde_json::json!({"username": "single-use-user", "password": "firstpass1"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
}

/// The password-reset endpoints are rate limited, like login and
/// registration. They are unauthenticated and each request for an
/// existing account writes a token row and a log line, so an unthrottled
/// endpoint lets a script grow the table and the log at request rate.
#[tokio::test]
async fn password_reset_rate_limit_returns_429() {
    let (app, _guard) = common::test_app().await;
    use tower::ServiceExt;
    // Burst 1, refilling only once an hour — so the second request is
    // refused without the test racing the clock.
    let router = app.router_with_limiters(
        gita_api::build_limiter(100, 200),
        gita_api::build_limiter(100, 200),
        gita_api::build_slow_refill_limiter(1),
    );

    let make_request = |router: axum::Router| async move {
        let req = axum::http::Request::builder()
            .method(axum::http::Method::POST)
            .uri("/api/auth/password-reset/request")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&serde_json::json!({"identifier": "someone"})).unwrap(),
            ))
            .unwrap();
        router.oneshot(req).await.unwrap().status()
    };

    let first = make_request(router.clone()).await;
    assert_eq!(
        first,
        axum::http::StatusCode::OK,
        "the first request should go through"
    );

    let second = make_request(router.clone()).await;
    assert_eq!(
        second,
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "the second request must be throttled"
    );
}

#[tokio::test]
async fn password_reset_flow() {
    let (app, guard) = common::test_app().await;
    let schema = guard.schema.clone();
    let _user = register_user(&app, "reset-user").await;

    // Reset request by username — should succeed silently even if no user matches.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/request",
        Some(serde_json::json!({"identifier": "reset-user"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Same identifier via email.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/request",
        Some(serde_json::json!({"identifier": "reset-user@e2e.local"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Non-existent user — still 200 (no enumeration).
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/request",
        Some(serde_json::json!({"identifier": "no-such-user"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Find the freshly-issued token directly in the DB (the production
    // path would have e-mailed the link; here we read it back for the test).
    let pool = common::test_db_url();
    let token: String = {
        let pg = sqlx::PgPool::connect(&pool).await.unwrap();
        let row: (String,) = sqlx::query_as(&format!(
            "SELECT token FROM \"{schema}\".password_reset_tokens WHERE used = FALSE
             ORDER BY created_at DESC LIMIT 1"
        ))
        .fetch_one(&pg)
        .await
        .unwrap();
        row.0
    };

    // Confirm with a new password.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/confirm",
        Some(serde_json::json!({"token": token, "new_password": "newpass99"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "confirm: {body}");

    // Old password no longer works.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/login",
        Some(serde_json::json!({"username": "reset-user", "password": "hunter22"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);

    // New password works.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/login",
        Some(serde_json::json!({"username": "reset-user", "password": "newpass99"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Reusing the same token is rejected.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/confirm",
        Some(serde_json::json!({"token": token, "new_password": "another2"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    // Invalid token -> 404.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/confirm",
        Some(serde_json::json!({"token": "deadbeef", "new_password": "another2"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    // Short new password -> 400.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/password-reset/confirm",
        Some(serde_json::json!({"token": "anything", "new_password": "12"})),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
}
