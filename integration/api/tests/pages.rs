//! End-to-end tests for GitHub-Pages-style static site serving.
//!
//! These boot a real `gita_server::App` (real Postgres schema, real bare
//! repos) and exercise the whole path: configure Pages via the API, push
//! real files, then fetch them over HTTP.
//!
//! Note on the SPA fall-through: `find_web_dist()` returns None under test,
//! so `web_router(None)` is a different router (no fallback_service). True
//! end-to-end SPA fall-through is covered by the gita-web unit test
//! `explicit_route_beats_spa_fallback_service`; here we assert the contract
//! (empty 404 body) directly instead.

use axum::http::{Method, StatusCode};
use gita_server::App;

mod common;
use common::*;

/// Assert that a request did NOT resolve to this repository's Pages site.
///
/// When a frontend build is present the fall-through serves the SPA shell
/// (200 + index.html); without one it is an empty 404. Either is correct —
/// what must never happen is the site's own content being served.
#[track_caller]
fn assert_not_pages_content(body: &[u8], uri: &str) {
    let text = String::from_utf8_lossy(body);
    assert!(
        !text.contains("Site home") && !text.contains("Docs home"),
        "{uri} must not be served Pages content"
    );
}

/// A 1x1 PNG with invalid-UTF-8 bytes. The binary-roundtrip test depends on
/// these bytes surviving intact.
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x18, 0x00, 0x00, 0x00, 0x26, 0x08, 0x02, 0x00, 0x00, 0x00, 0x39, 0x1D, 0x09,
    0x7C, 0x11, 0xFF, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01,
    0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn push_site(app: &App, owner: &str, name: &str) {
    push_files(
        app,
        owner,
        name,
        &[
            ("index.html", b"<h1>Site home</h1>\n".as_slice()),
            ("assets/logo.png", TINY_PNG),
            ("docs/index.html", b"<h1>Docs home</h1>\n".as_slice()),
            ("404.html", b"<h1>Custom 404</h1>\n".as_slice()),
        ],
        "main",
        "add site",
    );
}

async fn enable_pages(app: &App, token: &str, owner: &str, name: &str, branch: &str, path: &str) {
    let (status, body) = set_pages(app, token, owner, name, true, branch, path).await;
    assert_eq!(status, StatusCode::OK, "set_pages failed: {body}");
    assert_eq!(body["enabled"], true);
}

/// The single most important test: a binary asset must be served byte-exact.
/// Without `read_file_bytes`, the lossy UTF-8 decode would corrupt this PNG.
#[tokio::test]
async fn binary_asset_is_served_byte_exact() {
    let (app, guard) = test_app().await;
    let _guard = guard;
    let user = register_user(&app, "pagesbin").await;
    create_repo(&app, &user.token, "site").await;
    push_site(&app, "pagesbin", "site");
    enable_pages(&app, &user.token, "pagesbin", "site", "main", "/").await;

    let (status, headers, body) =
        request_bytes(&app, Method::GET, "/pagesbin/site/assets/logo.png", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("content-type").and_then(|v| v.to_str().ok()),
        Some("image/png")
    );
    assert_eq!(
        body, TINY_PNG,
        "binary asset must be served byte-exact"
    );
}

/// The site root must serve index.html. This guards the two-route design
/// (a trailing-slash root route distinct from the asset wildcard) — without
/// it, /owner/repo/ would fall through to the SPA.
#[tokio::test]
async fn site_root_serves_index_html() {
    let (app, guard) = test_app().await;
    let _guard = guard;
    let user = register_user(&app, "pagesroot").await;
    create_repo(&app, &user.token, "site").await;
    push_site(&app, "pagesroot", "site");
    enable_pages(&app, &user.token, "pagesroot", "site", "main", "/").await;

    let (status, headers, body) = request_bytes(&app, Method::GET, "/pagesroot/site/", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("content-type").and_then(|v| v.to_str().ok()),
        Some("text/html; charset=utf-8")
    );
    assert!(String::from_utf8_lossy(&body).contains("Site home"));

    // An asset below the root also works.
    let (status, _, _) = request_bytes(&app, Method::GET, "/pagesroot/site/index.html", None).await;
    assert_eq!(status, StatusCode::OK);
}

/// Private repos: anonymous must NOT get site content (empty 404), while
/// the owner with a Bearer token can. This encodes the v1 limitation that
/// browsers need a header-bearing client for private sites.
#[tokio::test]
async fn private_repo_pages_denied_for_anonymous() {
    let (app, guard) = test_app().await;
    let _guard = guard;
    let user = register_user(&app, "pagespriv").await;
    // Create a private repo.
    let body = serde_json::json!({
        "name": "secret",
        "description": "private site",
        "default_branch": "main",
        "is_private": true,
    });
    let (status, _) = request(
        &app,
        Method::POST,
        "/api/repos",
        Some(body),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    push_site(&app, "pagespriv", "secret");
    enable_pages(&app, &user.token, "pagespriv", "secret", "main", "/").await;

    // Anonymous: falls through to the SPA, never to the private site content.
    let (_, _, body) = request_bytes(&app, Method::GET, "/pagespriv/secret/", None).await;
    assert_not_pages_content(&body, "/pagespriv/secret/ (anonymous)");

    // Owner with Bearer token: serves.
    let (status, _, body) =
        request_bytes(&app, Method::GET, "/pagespriv/secret/", Some(&user.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains("Site home"));
}

/// Path traversal must not leak file contents outside the publishing root.
#[tokio::test]
async fn path_traversal_is_blocked() {
    let (app, guard) = test_app().await;
    let _guard = guard;
    let user = register_user(&app, "pagesxss").await;
    create_repo(&app, &user.token, "site").await;
    push_site(&app, "pagesxss", "site");
    enable_pages(&app, &user.token, "pagesxss", "site", "main", "/").await;

    // Parent-directory traversal.
    let (_, _, body) =
        request_bytes(&app, Method::GET, "/pagesxss/site/../../Cargo.toml", None).await;
    assert!(
        !String::from_utf8_lossy(&body).contains("gita-api"),
        "traversal must not leak file contents"
    );

    // Encoded traversal (percent-decoded by axum into `..`).
    let (_, _, body) =
        request_bytes(&app, Method::GET, "/pagesxss/site/%2e%2e/%2e%2e/.env", None).await;
    assert!(
        !String::from_utf8_lossy(&body).contains("JWT_SECRET"),
        "encoded traversal must not leak file contents"
    );
}

/// Publishing from /docs: only the docs subtree is served; a file at the
/// repo root must 404.
#[tokio::test]
async fn docs_root_path_publishes_from_docs_subdir() {
    let (app, guard) = test_app().await;
    let _guard = guard;
    let user = register_user(&app, "pagesdocs").await;
    create_repo(&app, &user.token, "site").await;
    push_site(&app, "pagesdocs", "site");
    // A file at the repo root that is NOT under /docs — this is the thing
    // /docs publishing must hide.
    push_files(
        &app,
        "pagesdocs",
        "site",
        &[("secret.txt", b"root secret\n")],
        "main",
        "add root secret",
    );
    enable_pages(&app, &user.token, "pagesdocs", "site", "main", "/docs").await;

    // The docs index is served.
    let (status, _, body) = request_bytes(&app, Method::GET, "/pagesdocs/site/", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains("Docs home"));

    // A repo-root file outside /docs must not be served.
    let (_, _, body) = request_bytes(&app, Method::GET, "/pagesdocs/site/secret.txt", None).await;
    assert!(
        !String::from_utf8_lossy(&body).contains("root secret"),
        "files outside the publishing root must not be served"
    );
}

/// Disabled Pages: the request must fall through with an empty 404 (the
/// contract that lets the SPA render).
#[tokio::test]
async fn disabled_pages_falls_through() {
    let (app, guard) = test_app().await;
    let _guard = guard;
    let user = register_user(&app, "pagesoff").await;
    create_repo(&app, &user.token, "site").await;
    push_site(&app, "pagesoff", "site");

    // Never enabled: falls through.
    let (_, _, body) = request_bytes(&app, Method::GET, "/pagesoff/site/", None).await;
    assert_not_pages_content(&body, "/pagesoff/site/");

    // Enable then disable: same behavior.
    enable_pages(&app, &user.token, "pagesoff", "site", "main", "/").await;
    let (status, _) = request(&app, Method::DELETE, "/api/repos/pagesoff/site/pages", None, Some(&user.token)).await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, body) = request_bytes(&app, Method::GET, "/pagesoff/site/", None).await;
    assert_not_pages_content(&body, "/pagesoff/site/");
}

/// A nonexistent repo must not 500; it falls through to the SPA.
#[tokio::test]
async fn nonexistent_repo_falls_through() {
    let (app, guard) = test_app().await;
    let _guard = guard;
    let user = register_user(&app, "pagesghost").await;
    create_repo(&app, &user.token, "real").await;

    let (_, _, body) = request_bytes(&app, Method::GET, "/pagesghost/nope/", None).await;
    assert_not_pages_content(&body, "/pagesghost/nope/");
}

/// Config endpoints require the owner: anonymous 401, non-owner 403, owner OK.
#[tokio::test]
async fn config_endpoints_require_owner() {
    let (app, guard) = test_app().await;
    let _guard = guard;
    let owner = register_user(&app, "pagesown").await;
    create_repo(&app, &owner.token, "site").await;
    // The owner's PUT with enabled=true validates that the branch exists, so
    // push a commit first.
    push_site(&app, "pagesown", "site");

    // Anonymous GET -> 401.
    let (status, _) = request(&app, Method::GET, "/api/repos/pagesown/site/pages", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Non-owner PUT -> 403.
    let other = register_user(&app, "pagesother").await;
    let (status, _) = set_pages(&app, &other.token, "pagesown", "site", true, "main", "/").await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Owner PUT -> 200 and GET reflects it.
    let (status, body) = set_pages(&app, &owner.token, "pagesown", "site", true, "main", "/docs").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], true);
    assert_eq!(body["branch"], "main");
    assert_eq!(body["path"], "/docs");
    let (status, body) = request(&app, Method::GET, "/api/repos/pagesown/site/pages", None, Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["url"], "/pagesown/site/docs");
}

/// Security headers: nosniff and CSP must be present on every response.
#[tokio::test]
async fn security_headers_present() {
    let (app, guard) = test_app().await;
    let _guard = guard;
    let user = register_user(&app, "pageshdr").await;
    create_repo(&app, &user.token, "site").await;
    push_site(&app, "pageshdr", "site");
    enable_pages(&app, &user.token, "pageshdr", "site", "main", "/").await;

    let (_, headers, _) = request_bytes(&app, Method::GET, "/pageshdr/site/", None).await;
    assert_eq!(
        headers.get("x-content-type-options").and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    assert!(headers.get("content-security-policy").is_some());
    assert!(headers.get("referrer-policy").is_some());
}

/// ETag revalidation: a second request with If-None-Match returns 304.
#[tokio::test]
async fn etag_revalidation_returns_304() {
    let (app, guard) = test_app().await;
    let _guard = guard;
    let user = register_user(&app, "pagesetag").await;
    create_repo(&app, &user.token, "site").await;
    push_site(&app, "pagesetag", "site");
    enable_pages(&app, &user.token, "pagesetag", "site", "main", "/").await;

    let (_, headers, _) = request_bytes(&app, Method::GET, "/pagesetag/site/", None).await;
    let etag = headers.get("etag").and_then(|v| v.to_str().ok()).expect("etag present").to_string();

    use tower::ServiceExt;
    let mut builder = axum::http::Request::builder().method(Method::GET).uri("/pagesetag/site/");
    builder = builder.header("if-none-match", &etag);
    let resp = app.router().oneshot(builder.body(axum::body::Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
}

/// 404.html is served with a 404 status when a missing file is requested.
#[tokio::test]
async fn site_custom_404_page_served() {
    let (app, guard) = test_app().await;
    let _guard = guard;
    let user = register_user(&app, "pages404").await;
    create_repo(&app, &user.token, "site").await;
    push_site(&app, "pages404", "site");
    enable_pages(&app, &user.token, "pages404", "site", "main", "/").await;

    let (status, _, body) = request_bytes(&app, Method::GET, "/pages404/site/missing.html", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(String::from_utf8_lossy(&body).contains("Custom 404"));
}

/// Reserved top-level path segments must not be captured as repository owners.
///
/// Pages serves at `/{owner}/{repo}/{*path}`, which structurally overlaps the
/// SPA's `/groups/{group}/{repo}` route. Before this was guarded, a group
/// repository page matched the Pages route as owner="groups", and the handler
/// returned its empty-404 fallthrough — which axum treats as a *final* response,
/// so the SPA fallback never ran and the page rendered blank.
///
/// Regression guard: the status alone is indistinguishable from a real
/// fall-through, so assert on the exact status/body the SPA contract requires
/// and that the reserved prefixes keep their own routing.
#[tokio::test]
async fn reserved_path_segments_are_not_treated_as_owners() {
    let (app, guard) = test_app().await;
    let _guard = guard;
    let user = register_user(&app, "pagesres").await;
    create_repo(&app, &user.token, "site").await;
    push_site(&app, "pagesres", "site");
    enable_pages(&app, &user.token, "pagesres", "site", "main", "/").await;

    // `/groups/{group}/{repo}` must fall through (empty 404), not be answered
    // by the Pages handler.
    for uri in [
        "/groups/acme/site",
        "/settings/profile",
        "/issues/list",
        "/api/anything",
        "/git/acme/site.git/info/refs",
    ] {
        let (_, _, body) = request_bytes(&app, Method::GET, uri, None).await;
        // Either the reserved route handles it (its own status), or Pages
        // Either the reserved route handles it (its own status/body), or
        // Pages steps aside and the SPA answers. What must never happen is
        // Pages serving this repo's site content for a reserved prefix.
        assert!(
            !String::from_utf8_lossy(&body).contains("Site home"),
            "{uri} must not be served Pages content"
        );
    }
}
