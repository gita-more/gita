//! Git over HTTP, end to end.
//!
//! The smart-HTTP handler decides who may read and who may write a
//! repository. What it answers — and what it answers *with* — is what
//! git clients act on:
//!
//! * a repository that needs credentials is a `401` that carries a
//!   `WWW-Authenticate: Basic` challenge, so the client asks the user
//!   for them and retries;
//! * a credential that names the wrong user is not a credential.

mod common;

use axum::http::{header, Method, StatusCode};
use base64::Engine;
use tower::ServiceExt;

/// A GET of the smart-HTTP info/refs endpoint.
async fn info_refs(app: &gita_admin::App, path: &str, auth: Option<String>) -> axum::http::Response<axum::body::Body> {
    let mut builder = axum::http::Request::builder()
        .method(Method::GET)
        .uri(format!("{path}?service=git-upload-pack"));
    if let Some(value) = auth {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    let req = builder.body(axum::body::Body::empty()).unwrap();
    app.router().oneshot(req).await.unwrap()
}

/// The value of a Basic credential: `username:token`, encoded.
fn basic(username: &str, token: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{username}:{token}"))
    )
}

/// A private repository without credentials is a 401 that asks for
/// Basic credentials — that challenge is what makes git prompt.
#[tokio::test]
async fn a_private_repository_challenges_anonymous_readers() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "gitauth").await;
    common::create_repo_with(
        &app,
        &user.token,
        "secret",
        Some(serde_json::json!({ "is_private": true })),
    )
    .await;

    let resp = info_refs(&app, "/gitauth/secret.git/info/refs", None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let challenge = resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .expect("the 401 must carry a WWW-Authenticate challenge");
    assert_eq!(
        challenge, "Basic realm=\"gita\"",
        "git clients only prompt when the challenge says Basic"
    );
}

/// A credential that is not valid does not break a public read: it is
/// treated as no credential at all.
///
/// The REST API refuses an invalid token outright; the git transport
/// answers an unauthenticated read of a public repository instead, so
/// that a client whose stored credentials have expired can still clone
/// what it could clone without them.
#[tokio::test]
async fn an_invalid_credential_is_treated_as_anonymous_on_a_public_read() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "gitauth").await;
    common::create_repo(&app, &user.token, "public").await;

    // A token that is not one the server issued.
    let resp = info_refs(
        &app,
        "/gitauth/public.git/info/refs",
        Some(basic("gitauth", "not-a-real-token")),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // A header that is not even base64.
    let resp = info_refs(
        &app,
        "/gitauth/public.git/info/refs",
        Some("Basic !!!not base64!!!".to_string()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // A Basic credential without the colon that separates the username
    // from the token.
    let encoded = base64::engine::general_purpose::STANDARD.encode("no-colon-here");
    let resp = info_refs(
        &app,
        "/gitauth/public.git/info/refs",
        Some(format!("Basic {encoded}")),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // On a private repository, the same garbage is refused with the
    // challenge, exactly as if no credential had been sent.
    common::create_repo_with(
        &app,
        &user.token,
        "secret",
        Some(serde_json::json!({ "is_private": true })),
    )
    .await;
    let resp = info_refs(
        &app,
        "/gitauth/secret.git/info/refs",
        Some("Basic !!!not base64!!!".to_string()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let challenge = resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .expect("the 401 must carry a WWW-Authenticate challenge");
    assert_eq!(challenge, "Basic realm=\"gita\"");
}

/// A credential that belongs to somebody else does not become a
/// credential for this user by naming them.
#[tokio::test]
async fn a_token_cannot_be_presented_under_another_users_name() {
    let (app, _guard) = common::test_app().await;
    let alice = common::register_user(&app, "alicegh").await;
    let mallory = common::register_user(&app, "mallorygh").await;
    common::create_repo_with(
        &app,
        &alice.token,
        "secret",
        Some(serde_json::json!({ "is_private": true })),
    )
    .await;

    // Mallory presents Alice's name with Mallory's own token.
    let resp = info_refs(
        &app,
        "/alicegh/secret.git/info/refs",
        Some(basic("alicegh", &mallory.token)),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "the username must name the user the token belongs to"
    );
}

/// The owner of a private repository reads it.
#[tokio::test]
async fn the_owner_reads_a_private_repository() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "gitowner").await;
    common::create_repo_with(
        &app,
        &user.token,
        "secret",
        Some(serde_json::json!({ "is_private": true })),
    )
    .await;

    let resp = info_refs(
        &app,
        "/gitowner/secret.git/info/refs",
        Some(basic("gitowner", &user.token)),
    )
    .await;
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    assert_eq!(status, StatusCode::OK, "got {}", String::from_utf8_lossy(&body));
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("git-upload-pack"),
        "the smart-HTTP advertisement must name the service, got: {body}"
    );
}

/// A repository that does not exist is a 404 — the same answer the
/// REST API gives, so the transports agree on what is missing.
///
/// This deliberately differs from the private-repository answer: a
/// private repository exists, so it challenges (401); a missing one
/// reports that nothing is there.
#[tokio::test]
async fn a_missing_repository_is_not_found() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "gitmissing").await;

    let resp = info_refs(&app, "/gitmissing/ghost.git/info/refs", None).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = info_refs(
        &app,
        "/gitmissing/ghost.git/info/refs",
        Some(basic("gitmissing", &user.token)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// A path that is not a git path is not intercepted: the router is not
/// a catch-all.
#[tokio::test]
async fn a_path_without_a_git_segment_is_not_intercepted() {
    let (app, _guard) = common::test_app().await;

    // The API answers; the git middleware passed the request through.
    let (status, _) = common::request(
        &app,
        Method::GET,
        "/api/repos",
        None,
        None,
    )
    .await;
    assert_ne!(status, StatusCode::UNAUTHORIZED, "got {status}");
}
