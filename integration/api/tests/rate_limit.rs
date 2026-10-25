mod common;


#[tokio::test]
async fn login_rate_limit_returns_429() {
    let (app, _guard) = common::test_app().await;
    // Default router uses a 100/200 limiter (effectively off) — install a
    // tighter one (burst 1, refilling only once an hour) so we can
    // demonstrate 429 without racing the clock.
    use tower::ServiceExt;
    let limiter = gita_api::build_slow_refill_limiter(1);
    let router = app.router_with_limiter(limiter);
    // Single request consumes the only token; the next must be 429.
    let req = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({
                "username": "nobody",
                "password": "nope",
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert!(
        resp.status() == axum::http::StatusCode::UNAUTHORIZED
            || resp.status() == axum::http::StatusCode::TOO_MANY_REQUESTS,
        "first request should be UNAUTHORIZED or already throttled, got {}",
        resp.status()
    );

    let req2 = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({
                "username": "nobody",
                "password": "nope",
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp2 = router.oneshot(req2).await.unwrap();
    assert_eq!(
        resp2.status(),
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "second request must hit the limit"
    );
}

/// Registration is throttled per IP with a bucket of its own — sharing the
/// login bucket would let login traffic starve registration and vice versa.
///
/// Every registration performs an Argon2 hash and creates a row, so an
/// unthrottled endpoint here is an unbounded user-creation primitive.
#[tokio::test]
async fn register_rate_limit_returns_429() {
    let (app, _guard) = common::test_app().await;
    use tower::ServiceExt;
    // Burst 1, refilling only once an hour — so the second registration is
    // refused without the test racing the clock.
    let router = app.router_with_limiters(
        gita_api::build_limiter(100, 200),
        gita_api::build_slow_refill_limiter(1),
        gita_api::build_limiter(100, 200),
    );

    let body = serde_json::json!({
        "username": "rate-user",
        "email": "rate-user@e2e.local",
        "password": "hunter22",
    });
    let req = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri("/api/auth/register")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "first registration should succeed"
    );

    let body2 = serde_json::json!({
        "username": "rate-user-2",
        "email": "rate-user-2@e2e.local",
        "password": "hunter22",
    });
    let req2 = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri("/api/auth/register")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body2).unwrap()))
        .unwrap();
    let resp2 = router.oneshot(req2).await.unwrap();
    assert_eq!(
        resp2.status(),
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "second registration must hit the limit"
    );
}

/// The login and registration buckets are independent: exhausting one must
/// not throttle the other.
#[tokio::test]
async fn login_and_register_use_separate_buckets() {
    let (app, _guard) = common::test_app().await;
    use tower::ServiceExt;
    // A single-token login bucket and a single-token register bucket, both
    // refilling only once an hour.
    let router = app.router_with_limiters(
        gita_api::build_slow_refill_limiter(1),
        gita_api::build_slow_refill_limiter(1),
        gita_api::build_limiter(100, 200),
    );

    // Burn the login bucket.
    let login = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({
                "username": "nobody",
                "password": "nope",
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp = router.clone().oneshot(login).await.unwrap();
    assert_ne!(resp.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);

    // Registration must still be allowed.
    let reg = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri("/api/auth/register")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({
                "username": "sep-user",
                "email": "sep-user@e2e.local",
                "password": "hunter22",
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp2 = router.clone().oneshot(reg).await.unwrap();
    assert_eq!(
        resp2.status(),
        axum::http::StatusCode::OK,
        "registration must not be throttled by login traffic"
    );

    // And now the login bucket is empty while registration still has its
    // second token (burst was 1, so it is empty too) — a second login is
    // refused.
    let login2 = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({
                "username": "nobody",
                "password": "nope",
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp3 = router.oneshot(login2).await.unwrap();
    assert_eq!(
        resp3.status(),
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "second login must hit the limit"
    );
}

/// The rate-limit key must not be forgeable by the client.
///
/// The peer address is authoritative when the request arrived over a
/// socket. Without one, the `X-Forwarded-For` fallback uses the *right*-most
/// entry: a client that prepends forged hops to the header must not be able
/// to mint a fresh bucket per request.
#[tokio::test]
async fn rate_limit_key_ignores_forged_forwarded_for_hops() {
    use tower::ServiceExt;
    let (app, _guard) = common::test_app().await;
    let router = app.router_with_limiters(
        gita_api::build_limiter(100, 200),
        gita_api::build_slow_refill_limiter(1),
        gita_api::build_limiter(100, 200),
    );

    // The first request carries a forged left-most hop and consumes the
    // bucket. The second carries a *different* forged left-most hop — the
    // right-most entry is unchanged, so it must land in the same bucket.
    let mut statuses = Vec::new();
    for forged in ["1.2.3.4", "5.6.7.8"] {
        let req = axum::http::Request::builder()
            .method(axum::http::Method::POST)
            .uri("/api/auth/register")
            .header("content-type", "application/json")
            .header("x-forwarded-for", format!("{forged}, 203.0.113.9"))
            .body(axum::body::Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "username": format!("xff-{forged}"),
                    "email": format!("xff-{forged}@e2e.local"),
                    "password": "hunter22",
                }))
                .unwrap(),
            ))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        statuses.push(resp.status());
    }

    assert_eq!(
        statuses[0],
        axum::http::StatusCode::OK,
        "first registration should succeed"
    );
    assert_eq!(
        statuses[1],
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "forging a left-most forwarded hop must not reset the bucket"
    );
}
