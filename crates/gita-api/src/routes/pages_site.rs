//! Serving of GitHub-Pages-style static sites.
//!
//! A repository with Pages enabled publishes the tree at `branch`/`path`
//! (path is `/` or `/docs`) verbatim at `/{owner}/{repo}/` — the
//! path-prefix shape of a GitHub *project* site.
//!
//! Routing model. The server's SPA fallback (gita-web) rewrites any
//! extensionless path to index.html, so Pages must be registered as explicit
//! routes — which always win over a fallback service. A request that is *not*
//! Pages content must fall through to the SPA by returning an empty-bodied
//! 404; a rendered 404 page would break every non-Pages repo.
//!
//! Privacy. Private-repo sites require an `Authorization: Bearer` header in
//! v1 — browsers do not send one on navigation, and this codebase has no cookie
//! session (auth is Bearer-only throughout). An unauthenticated request to a
//! private site falls through to the SPA. v2 options: cookie auth, or an
//! opt-in per-repo pages token. A `?token=` query parameter was rejected
//! because it would leak into browser history, server access logs, and
//! `Referer` headers.

use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use gita_core::Repository;

/// Options for the Pages serving router.
#[derive(Debug, Clone, Default)]
pub struct PagesOptions {
    /// Optional URL prefix for Pages sites, e.g. `"/p"` serves
    /// `/p/owner/repo/`. Empty (the default) serves at `/owner/repo/`,
    /// mirroring GitHub project sites.
    pub url_prefix: String,
}

/// Build the Pages serving routes.
///
/// This is a separate router from `pages_routes()` because it must be mounted
/// at the server root — a static site is reachable at `/{owner}/{repo}/`, not
/// under `/api`.
pub fn pages_site_routes(opts: PagesOptions) -> Router<AppState> {
    let prefix = opts.url_prefix.trim_matches('/');
    let (root, assets) = if prefix.is_empty() {
        (
            "/{owner}/{repo}/".to_string(),
            "/{owner}/{repo}/{*path}".to_string(),
        )
    } else {
        (
            format!("/{prefix}/{{owner}}/{{repo}}/"),
            format!("/{prefix}/{{owner}}/{{repo}}/{{*path}}"),
        )
    };
    Router::new()
        .route(&root, get(serve_pages_root))
        .route(&assets, get(serve_pages_asset))
}

/// Serve the site root (`/owner/repo/`).
///
/// axum 0.8's `{*path}` wildcard does not match an empty segment, so
/// `/{owner}/{repo}/{*path}` does not match `/owner/repo/`. The root needs its
/// own route, or the first URL a visitor tries falls through to the SPA.
async fn serve_pages_root(
    State(state): State<AppState>,
    Path((owner, repo)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    serve_site_request(&state, &owner, &repo, "", &headers).await
}

/// Serve an asset below the site root (`/owner/repo/path/to/file`).
async fn serve_pages_asset(
    State(state): State<AppState>,
    Path((owner, repo, path)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    serve_site_request(&state, &owner, &repo, &path, &headers).await
}

/// Core serving logic shared by the root and asset routes.
async fn serve_site_request(
    state: &AppState,
    owner: &str,
    repo_name: &str,
    raw_path: &str,
    headers: &HeaderMap,
) -> Response {
    // Load the repository and its Pages config. Any failure here means this
    // URL is not a Pages site: fall through to the SPA with an empty 404.
    let repository = match state.db.get_repository_by_name(owner, repo_name).await {
        Ok(Some(r)) => r,
        _ => return spa_fallthrough(),
    };
    let row = match state.db.get_pages(repository.id).await {
        Ok(Some(r)) if r.enabled => r,
        _ => return spa_fallthrough(),
    };

    // Private repos: the owner (only) may read the site, and only when
    // authenticated. Anonymous requests fall through so the repo page's own
    // login gate handles the redirect.
    if repository.is_private && !authed_owner_matches(state, headers, &repository).await {
        return spa_fallthrough();
    }

    // Resolve the publishing root. `path` is constrained to "/" or "/docs" at
    // config time, so this is safe by construction; a defensive check keeps
    // a corrupted DB row from becoming a traversal.
    let base = row.path.trim_matches('/');
    let site_path = match sanitize_site_path(raw_path) {
        Some(p) => p,
        None => return spa_fallthrough(),
    };

    let full_path = if base.is_empty() {
        site_path.clone()
    } else {
        format!("{base}/{site_path}")
    };

    // Resolve the branch to a commit. A branch deleted after config time
    // should fall through rather than 500.
    let branch = row.branch.clone();
    let resolved = state
        .git
        .resolve_branch(owner, repo_name, &branch)
        .ok()
        .filter(|sha| state.git.ref_exists(owner, repo_name, sha));
    let Some(ref_sha) = resolved else {
        return spa_fallthrough();
    };

    // If the requested path is a directory or ends with '/', serve index.html.
    let is_dir = full_path.ends_with('/') || looks_like_directory(state, owner, repo_name, &ref_sha, &full_path);
    let target_path = if is_dir {
        format!("{}/index.html", full_path.trim_end_matches('/'))
    } else {
        full_path.clone()
    };
    let target_path = target_path.trim_start_matches('/').to_string();

    let blob = match state
        .git
        .read_file_bytes(owner, repo_name, &ref_sha, &target_path)
    {
        Ok(Some(b)) => b,
        Ok(None) => return serve_not_found_or_fallthrough(state, owner, repo_name, &ref_sha, base).await,
        Err(_) => return spa_fallthrough(),
    };

    let ct = content_type_for(&target_path);
    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, ct)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, cache_control_for(&target_path))
        .header("Referrer-Policy", "no-referrer")
        .header("Content-Security-Policy", content_security_policy())
        .header(header::ETAG, format!("\"{}\"", blob.sha));

    // ETag revalidation: an unchanged blob returns 304 with no body.
    if let Some(if_none_match) = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) {
        if if_none_match == format!("\"{}\"", blob.sha) {
            return builder
                .status(StatusCode::NOT_MODIFIED)
                .body(Body::empty())
                .unwrap_or_else(|_| spa_fallthrough());
        }
    }

    builder
        .body(Body::from(blob.data))
        .unwrap_or_else(|_| spa_fallthrough())
}

/// Determine whether `full_path` is a directory by checking if the path (or
/// the path plus a slash) has children. Cheap check using list_tree on the
/// immediate parent.
fn looks_like_directory(
    state: &AppState,
    owner: &str,
    repo_name: &str,
    ref_sha: &str,
    full_path: &str,
) -> bool {
    if full_path.is_empty() {
        return true;
    }
    let parent = full_path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    let Ok(entries) = state.git.list_tree(owner, repo_name, ref_sha, parent) else {
        return false;
    };
    entries.iter().any(|e| {
        if e.kind == "tree" {
            e.path == full_path
        } else {
            false
        }
    })
}

/// A missing file: serve the site's own 404.html with status 404 if present;
/// otherwise fall through to the SPA with an empty 404.
///
/// `base` is the publishing root ("", "docs", ...) — GitHub Pages serves
/// `404.html` from the root of the publishing source, not next to the missing
/// file.
async fn serve_not_found_or_fallthrough(
    state: &AppState,
    owner: &str,
    repo_name: &str,
    ref_sha: &str,
    base: &str,
) -> Response {
    let not_found_path = if base.is_empty() {
        "404.html".to_string()
    } else {
        format!("{base}/404.html")
    };
    if let Ok(Some(blob)) = state.git.read_file_bytes(owner, repo_name, ref_sha, &not_found_path) {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .header(header::CACHE_CONTROL, "no-cache")
            .header("Referrer-Policy", "no-referrer")
            .header("Content-Security-Policy", content_security_policy())
            .body(Body::from(blob.data))
            .unwrap_or_else(|_| spa_fallthrough())
    } else {
        spa_fallthrough()
    }
}

/// The empty-bodied 404 that lets the SPA render. This is the contract the
/// whole URL scheme depends on.
fn spa_fallthrough() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::empty())
        .unwrap_or_default()
}

/// Resolve the caller against the Authorization header and check ownership of
/// a private repo. Mirrors gita-http's `authorize_git_request` for private
/// repos, but Bearer-only (browsers never send Basic on navigation).
async fn authed_owner_matches(
    state: &AppState,
    headers: &HeaderMap,
    repository: &Repository,
) -> bool {
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    match state.auth.resolve_bearer(&state.db, token).await {
        Ok(user) => user.id == repository.owner_id,
        Err(_) => false,
    }
}

/// Reject anything that could escape the publishing root.
///
/// Returns the cleaned relative path, or None when the input is hostile. Note
/// axum percent-decodes before `Path` extraction, so `%2e%2e` arrives as `..`
/// and is caught here. Git's `lookup_entry_by_path` does not follow symlinks
/// or normalize `..`, so this is primarily defense-in-depth against reaching a
/// path that a lookup might misinterpret.
fn sanitize_site_path(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return Some(String::new());
    }
    if raw.len() > 1024 {
        return None;
    }
    let path = raw.trim_start_matches('/');
    if path.contains('\\') || path.contains('\0') {
        return None;
    }
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() > 64 {
        return None;
    }
    for seg in &segments {
        if seg.is_empty() || *seg == "." || *seg == ".." {
            return None;
        }
        // Defense-in-depth: never expose .git* segments.
        if seg.starts_with(".git") {
            return None;
        }
    }
    Some(segments.join("/"))
}

/// Extension → Content-Type, lowercased. The default is application/octet-stream.
fn content_type_for(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" | "md" | "csv" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// Cache-Control strategy:
/// - HTML: `no-cache` so a new push propagates immediately (revalidate).
/// - Hash-like assets under known asset directories: long-lived immutable.
/// - Everything else: short public cache.
///
/// This is a heuristic because there is no build step and therefore no
/// manifest of hashed filenames. The asset-directory names (`assets/`,
/// `static/`, `dist/`) plus a hash-shaped segment make a wrong guess unlikely.
fn cache_control_for(path: &str) -> &'static str {
    if path.ends_with(".html") || path.ends_with(".htm") {
        return "no-cache";
    }
    let lower = path.to_ascii_lowercase();
    let in_asset_dir = ["/assets/", "/static/", "/dist/", "assets/", "static/", "dist/"]
        .iter()
        .any(|d| lower.contains(d));
    if in_asset_dir {
        return "public, max-age=31536000, immutable";
    }
    // hash-like segment: name-<8+hex>.ext
    if let Some(seg) = lower.rsplit('/').next() {
        if let Some((_, ext)) = seg.rsplit_once('.') {
            if !ext.is_empty() {
                if let Some((name, _)) = seg.split_once('.') {
                    if let Some(idx) = name.rfind('-') {
                        let tail = &name[idx + 1..];
                        if tail.len() >= 8 && tail.bytes().all(|b| b.is_ascii_hexdigit()) {
                            return "public, max-age=31536000, immutable";
                        }
                    }
                }
            }
        }
    }
    "public, max-age=300"
}

fn content_security_policy() -> &'static str {
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; object-src 'none'; frame-ancestors 'self'; base-uri 'self'"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_mapping() {
        assert_eq!(content_type_for("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type_for("style.css"), "text/css; charset=utf-8");
        assert_eq!(content_type_for("app.js"), "text/javascript; charset=utf-8");
        assert_eq!(content_type_for("logo.PNG"), "image/png");
        assert_eq!(content_type_for("favicon.ico"), "image/x-icon");
        assert_eq!(content_type_for("font.woff2"), "font/woff2");
        assert_eq!(content_type_for("data.json"), "application/json");
        assert_eq!(content_type_for("no-extension"), "application/octet-stream");
        assert_eq!(content_type_for("file.unknownext"), "application/octet-stream");
    }

    #[test]
    fn cache_control_mapping() {
        assert_eq!(cache_control_for("index.html"), "no-cache");
        assert_eq!(cache_control_for("docs/page.html"), "no-cache");
        assert_eq!(cache_control_for("assets/app-a1b2c3d4e5f6.js"), "public, max-age=31536000, immutable");
        assert_eq!(cache_control_for("static/style.css"), "public, max-age=31536000, immutable");
        assert_eq!(cache_control_for("data.json"), "public, max-age=300");
        assert_eq!(cache_control_for("app.js"), "public, max-age=300");
    }

    #[test]
    fn sanitize_path_table() {
        // Safe paths pass through normalized.
        assert_eq!(sanitize_site_path("index.html").as_deref(), Some("index.html"));
        assert_eq!(sanitize_site_path("a/b/c.png").as_deref(), Some("a/b/c.png"));
        assert_eq!(sanitize_site_path("/docs/index.html").as_deref(), Some("docs/index.html"));
        assert_eq!(sanitize_site_path("").as_deref(), Some(""));

        // Hostile paths are rejected. Percent-encoding is decoded by axum before
        // this function runs, so the encoded form reaches us as ".." and is
        // caught by the `..` rule above.
        assert!(sanitize_site_path("../etc/passwd").is_none());
        assert!(sanitize_site_path("a/../b").is_none());
        assert!(sanitize_site_path("a/./b").is_none());
        assert!(sanitize_site_path("a//b").is_none());
        assert!(sanitize_site_path("/").is_none());
        assert!(sanitize_site_path("a\\b").is_none());
        assert!(sanitize_site_path(".git/config").is_none());
        // Over-length / too many segments.
        assert!(sanitize_site_path(&"x".repeat(1025)).is_none());
        let deep = (0..65).map(|_| "d").collect::<Vec<_>>().join("/");
        assert!(sanitize_site_path(&deep).is_none());
    }
}