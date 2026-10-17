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

/// Serves static files from `web/dist` and falls back to a landing page.
pub fn web_router(dist_dir: Option<PathBuf>) -> Router {
    if let Some(dist) = dist_dir {
        if dist.join("index.html").exists() {
            // Serve the built frontend. Use ServeDir with precompressed support
            // and explicit index.html for root path.
            let serve_dir = ServeDir::new(&dist)
                .append_index_html_on_directories(true)
                .precompressed_gzip()
                .precompressed_br()
                .precompressed_deflate()
                .precompressed_zstd();

            return Router::new()
                .route("/", get(move || async move {
                    let index_path = dist.join("index.html");
                    match fs::read_to_string(&index_path) {
                        Ok(html) => Html(html).into_response(),
                        Err(_) => (
                            StatusCode::NOT_FOUND,
                            "index.html not found",
                        ).into_response(),
                    }
                }))
                .fallback_service(
                    get_service(serve_dir).handle_error(|_| async {
                        (
                            StatusCode::NOT_FOUND,
                            "not found",
                        )
                            .into_response()
                    }),
                );
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
}