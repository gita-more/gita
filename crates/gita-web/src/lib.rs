//! Web UI static file serving.
//!
//! Serves the built React frontend from `web/dist` if present.
//! Falls back to a minimal embedded landing page when the frontend
//! hasn't been built yet.

use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Router, routing::get_service};
use std::path::{Path, PathBuf};
use tower_http::services::ServeDir;

/// Serves static files from `web/dist` and falls back to a landing page.
pub fn web_router(dist_dir: Option<PathBuf>) -> Router {
    let mut router = Router::new();

    if let Some(dist) = dist_dir {
        if dist.join("index.html").exists() {
            router = router.nest_service(
                "/",
                get_service(ServeDir::new(&dist)).handle_error(|_| async {
                    (
                        StatusCode::NOT_FOUND,
                        "not found",
                    )
                        .into_response()
                }),
            );
            return router;
        }
    }

    // Fallback landing page (no frontend build yet).
    router = router.route("/", get(landing_page));
    router
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