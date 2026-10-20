//! Web UI static file serving.
//!
//! Serves the built React frontend from `web/dist` if present.
//! Falls back to a minimal embedded landing page when the frontend
//! hasn't been built yet.

use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::{Router, routing::get_service};
use std::fs;
use std::path::PathBuf;
use tower_http::services::ServeDir;

/// Serves static files from `web/dist` with SPA fallback. Falls back to a
/// landing page when the frontend hasn't been built yet.
pub fn web_router(dist_dir: Option<PathBuf>) -> Router {
    if let Some(dist) = dist_dir {
        if dist.join("index.html").exists() {
            let index_path = dist.join("index.html");
            // SPA mode: use ServeDir with a fallback that rewrites 404s for
            // HTML routes to index.html so client-side routes (e.g.
            // /groups, /:owner/:repo) work on direct navigation and page
            // refresh. Static assets fall through ServeDir normally;
            // non-HTML 404s still get a real 404.
            let serve_dir = ServeDir::new(&dist)
                .append_index_html_on_directories(true)
                .precompressed_gzip()
                .precompressed_br()
                .precompressed_deflate()
                .precompressed_zstd()
                .fallback(get(move |uri: axum::extract::OriginalUri| {
                    let index_path = index_path.clone();
                    async move {
                        let path = uri.0.path();
                        let looks_like_route = path
                            .split('?')
                            .next()
                            .map(|p| !p.contains('.') || p.ends_with(".html"))
                            .unwrap_or(false);
                        if !looks_like_route {
                            return (
                                StatusCode::NOT_FOUND,
                                "not found",
                            )
                                .into_response();
                        }
                        match fs::read_to_string(&index_path) {
                            Ok(html) => Html(html).into_response(),
                            Err(_) => (
                                StatusCode::NOT_FOUND,
                                "index.html not found",
                            )
                                .into_response(),
                        }
                    }
                }));

            return Router::new().fallback_service(get_service(serve_dir));
        }
    }

    // Fallback landing page (no frontend build yet).
    Router::new().route("/", get(landing_page))
}

async fn landing_page() -> Html<&'static str> {
    Html(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Gita — AI-native Git Server</title>
<style>
  body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif; background: #0d1117; color: #e6edf3; margin: 0; display: flex; align-items: center; justify-content: center; min-height: 100vh; }
  .card { max-width: 640px; padding: 40px; text-align: center; }
  h1 { font-size: 2.4rem; margin-bottom: 0.4rem; }
  p { color: #8b949e; line-height: 1.6; }
  code { background: #161b22; padding: 2px 6px; border-radius: 4px; color: #79c0ff; }
  .btn { display: inline-block; margin-top: 20px; padding: 10px 24px; border-radius: 6px; background: #238636; color: #fff; text-decoration: none; font-weight: 600; }
  .btn:hover { background: #2ea043; }
</style>
</head>
<body>
  <div class="card">
    <h1>🦀 Gita</h1>
    <p>AI-native Git server built with Rust.</p>
    <p>API is live at <code>/api/health</code>. The Web UI will appear here once the frontend is built (<code>cd web && npm install && npm run build</code>).</p>
    <a class="btn" href="/api/health">Check health</a>
  </div>
</body>
</html>"#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn fallback_router_serves_landing_page() {
        let app = web_router(None);
        let res = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(html.contains("Gita"));
    }

    #[tokio::test]
    async fn with_dist_serves_index_html() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<!DOCTYPE html><html><body>Hello dist</body></html>",
        )
        .unwrap();

        let app = web_router(Some(dir.path().to_path_buf()));
        let res = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(html.contains("Hello dist"));
    }

    #[tokio::test]
    async fn dist_without_index_falls_back_to_landing() {
        let dir = tempfile::TempDir::new().unwrap();
        // dist dir exists but has no index.html
        let app = web_router(Some(dir.path().to_path_buf()));
        let res = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(html.contains("Gita"));
    }

    #[tokio::test]
    async fn path_traversal_is_blocked() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<!DOCTYPE html><html><body>Hello dist</body></html>",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/app.js"), "console.log('x')").unwrap();

        let app = web_router(Some(dir.path().to_path_buf()));

        // Attempt to escape the dist directory via path traversal.
        let res = app
            .clone()
            .oneshot(Request::builder().uri("/../Cargo.toml").body(Body::empty()).unwrap())
            .await
            .unwrap();
        // ServeDir normalizes/denies traversal; should not be a 200 that
        // leaks file content.
        assert_ne!(
            res.status(),
            axum::http::StatusCode::OK,
            "path traversal must not return file contents"
        );
        // A well-formed deep request still works.
        let res = app
            .oneshot(Request::builder().uri("/assets/app.js").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_static_file_returns_404() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<!DOCTYPE html><html><body>Hi</body></html>",
        )
        .unwrap();
        let app = web_router(Some(dir.path().to_path_buf()));
        let res = app
            .oneshot(Request::builder().uri("/nope.png").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::NOT_FOUND);
    }

    /// SPA fallback: a path with no extension gets rewritten to index.html.
    #[tokio::test]
    async fn spa_fallback_serves_index_html_for_html_routes() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<!DOCTYPE html><html><body>SPA Index</body></html>",
        )
        .unwrap();
        let app = web_router(Some(dir.path().to_path_buf()));

        // /groups has no static file and no extension → SPA fallback.
        let res = app
            .clone()
            .oneshot(Request::builder().uri("/groups").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(html.contains("SPA Index"));

        // /owner/repo also works.
        let res = app
            .oneshot(Request::builder().uri("/owner/repo").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(html.contains("SPA Index"));
    }

    /// An explicit route must win over the SPA `fallback_service`, no matter
    /// the merge order. This is the mechanism Pages relies on: a repo with
    /// Pages enabled serves its own content while everything else falls
    /// through to the React app. Without an explicit route, axum consults the
    /// fallback service only after the path router misses — and the fallback
    /// rewrites *any* extensionless path to index.html, which would silently
    /// swallow a Pages site root like `/alice/docs/`.
    #[tokio::test]
    async fn explicit_route_beats_spa_fallback_service() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<!DOCTYPE html><html><body>SPA Index</body></html>",
        )
        .unwrap();
        let mut app = web_router(Some(dir.path().to_path_buf()));
        app = app.route(
            "/{owner}/{repo}/{*path}",
            axum::routing::get(|| async { "PAGES_CONTENT" }),
        );

        // The explicit route wins for Pages-shaped paths.
        let res = app
            .clone()
            .oneshot(Request::builder().uri("/alice/docs/index.html").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"PAGES_CONTENT");

        // Non-Pages paths still reach the SPA fallback.
        let res = app
            .oneshot(Request::builder().uri("/groups").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("SPA Index"));
    }
}