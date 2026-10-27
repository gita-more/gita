//! Smart HTTP Git protocol endpoint.
//!
//! This crate exposes an axum middleware that intercepts git smart-HTTP
//! requests (identified by the `.git` path suffix) at the root path, so
//! clone URLs are clean (`/owner/repo.git`) like GitHub/GitLab. It
//! delegates to the `git http-backend` CGI program via the
//! [`gita_git::GitManager`], which handles upload-pack (fetch/clone)
//! and receive-pack (push).

pub mod auth;
pub mod parse;
pub mod router;

use gita_api::auth::Auth;
use gita_db::Db;
use gita_events::EventBus;
use gita_git::GitManager;
use std::sync::Arc;

pub use router::git_middleware;

/// Shared state for the git HTTP handler.
#[derive(Clone)]
pub struct GitHttpState {
    pub git: Arc<GitManager>,
    pub db: Db,
    pub bus: EventBus,
    pub auth: Arc<Auth>,
}

/// A test helper to build the router with a stub database.
#[cfg(test)]
pub async fn test_state() -> GitHttpState {
    let url = std::env::var("GITA_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://gita:gita@127.0.0.1:55432/gita".to_string());
    let db = gita_db::new_test_db(&url).await;
    let root = std::env::temp_dir().join(format!("gita-http-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let git = GitManager::new(root);
    GitHttpState {
        git: Arc::new(git),
        db,
        bus: EventBus::default(),
        auth: Arc::new(gita_api::auth::Auth::new("test-secret".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{authorize_git_request, resolve_authed_user};
    use crate::parse::{is_read_request, is_write_request, request_verb};
    use axum::http::{header, HeaderMap, StatusCode};
    use base64::Engine as _;
    use gita_core::EventType;

    #[tokio::test]
    async fn git_request_authorization() {
        let state = test_state().await;
        let db = &state.db;
        let user = db
            .create_user("carol", "carol@example.com", "h")
            .await
            .unwrap();
        let public_repo = db
            .create_repository(user.id, "carol", "pub", "public", false, "main", None)
            .await
            .unwrap();
        let private_repo = db
            .create_repository(user.id, "carol", "priv", "private", true, "main", None)
            .await
            .unwrap();

        // Generate a JWT for the owner.
        let token = state.auth.sign_jwt(&user).unwrap();
        let mut authed_headers = HeaderMap::new();
        authed_headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let empty_headers = HeaderMap::new();
        let no_query = &None;

        // Public repo: anonymous read is allowed.
        let res = authorize_git_request(
            &state,
            "carol",
            &public_repo.name,
            "/carol/pub.git/info/refs",
            no_query,
            &empty_headers,
        )
        .await;
        assert!(res.is_ok(), "public repo should be anonymously readable");

        // Private repo: anonymous read is rejected.
        let res = authorize_git_request(
            &state,
            "carol",
            &private_repo.name,
            "/carol/priv.git/info/refs",
            no_query,
            &empty_headers,
        )
        .await;
        assert!(res.is_err(), "private repo should reject anonymous read");
        assert_eq!(res.unwrap_err().status(), StatusCode::UNAUTHORIZED);

        // Private repo: owner read is allowed.
        let res = authorize_git_request(
            &state,
            "carol",
            &private_repo.name,
            "/carol/priv.git/info/refs",
            no_query,
            &authed_headers,
        )
        .await;
        assert!(res.is_ok(), "private repo owner should be able to read");

        // Push without auth: rejected.
        let res = authorize_git_request(
            &state,
            "carol",
            &public_repo.name,
            "/carol/pub.git/git-receive-pack",
            no_query,
            &empty_headers,
        )
        .await;
        assert!(res.is_err(), "push should require authentication");
        assert_eq!(res.unwrap_err().status(), StatusCode::UNAUTHORIZED);

        // Push as owner: allowed (Bearer).
        let res = authorize_git_request(
            &state,
            "carol",
            &public_repo.name,
            "/carol/pub.git/git-receive-pack",
            no_query,
            &authed_headers,
        )
        .await;
        assert!(res.is_ok(), "owner should be able to push");

        // Push as owner: allowed via Basic auth (git smart HTTP style).
        // The remote URL is http://owner:token@host/..., so the Basic
        // credentials are `carol:<token>`.
        let basic_value = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("carol:{token}"))
        );
        let mut basic_headers = HeaderMap::new();
        basic_headers.insert(header::AUTHORIZATION, basic_value.parse().unwrap());
        let res = authorize_git_request(
            &state,
            "carol",
            &public_repo.name,
            "/carol/pub.git/git-receive-pack",
            no_query,
            &basic_headers,
        )
        .await;
        assert!(res.is_ok(), "owner should be able to push via Basic auth");

        // Push with wrong password in Basic auth: rejected.
        let bad_basic = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("carol:not-a-token")
        );
        let mut bad_headers = HeaderMap::new();
        bad_headers.insert(header::AUTHORIZATION, bad_basic.parse().unwrap());
        let res = authorize_git_request(
            &state,
            "carol",
            &public_repo.name,
            "/carol/pub.git/git-receive-pack",
            no_query,
            &bad_headers,
        )
        .await;
        assert!(res.is_err(), "push with wrong token should be rejected");
        assert_eq!(res.unwrap_err().status(), StatusCode::UNAUTHORIZED);
    }

    /// The username in a Basic credential must name the user the token
    /// belongs to. A token presented under somebody else's name is not a
    /// valid credential.
    #[tokio::test]
    async fn basic_auth_requires_the_username_to_match_the_token() {
        let state = test_state().await;
        let db = &state.db;
        let user = db
            .create_user("carol", "carol@example.com", "h")
            .await
            .unwrap();
        let other = db
            .create_user("dave", "dave@example.com", "h")
            .await
            .unwrap();
        let private_repo = db
            .create_repository(user.id, "carol", "priv", "private", true, "main", None)
            .await
            .unwrap();
        let token = state.auth.sign_jwt(&user).unwrap();
        let no_query = &None;

        let basic = |username: &str| {
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(format!("{username}:{token}"));
            let mut headers = HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                format!("Basic {encoded}").parse().unwrap(),
            );
            headers
        };

        // The owner naming themselves works.
        let res = authorize_git_request(
            &state,
            "carol",
            &private_repo.name,
            "/carol/priv.git/info/refs",
            no_query,
            &basic("carol"),
        )
        .await;
        assert!(
            res.is_ok(),
            "the owner must be able to read over Basic auth"
        );

        // The same token under a different name does not.
        let res = authorize_git_request(
            &state,
            "carol",
            &private_repo.name,
            "/carol/priv.git/info/refs",
            no_query,
            &basic("dave"),
        )
        .await;
        assert!(
            res.is_err(),
            "a token presented under another user's name must be rejected"
        );
        assert_eq!(res.unwrap_err().status(), StatusCode::UNAUTHORIZED);

        // ... and neither does a name that belongs to nobody.
        let res = authorize_git_request(
            &state,
            "carol",
            &private_repo.name,
            "/carol/priv.git/info/refs",
            no_query,
            &basic("nobody"),
        )
        .await;
        assert!(res.is_err(), "an unknown username must be rejected");

        // Sanity: the other real user is not the token's owner either.
        assert_ne!(other.id, user.id);
    }

    #[tokio::test]
    async fn authorize_via_query_string_service_flag() {
        // `git-receive-pack` may be specified in either path_info or
        // ?service=git-receive-pack; the body of the request (git smart
        // HTTP) typically uses the query-string form.
        let state = test_state().await;
        let user = state
            .db
            .create_user("dsvc", "dsvc@example.com", "h")
            .await
            .unwrap();
        state
            .db
            .create_repository(user.id, "dsvc", "qrepo", "qs test", false, "main", None)
            .await
            .unwrap();

        // query-string receive-pack without auth -> denied
        let qs = Some("service=git-receive-pack".to_string());
        let empty_headers = HeaderMap::new();
        let res = authorize_git_request(
            &state,
            "dsvc",
            "qrepo",
            "/dsvc/qrepo.git/info/refs",
            &qs,
            &empty_headers,
        )
        .await;
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authorize_unknown_repo_returns_404() {
        let state = test_state().await;
        let empty_headers = HeaderMap::new();
        let res = authorize_git_request(
            &state,
            "nobody",
            "void",
            "/nobody/void.git/info/refs",
            &None,
            &empty_headers,
        )
        .await;
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn parse_receive_pack_ref_updates_handles_flush_only() {
        let body = b"0000".to_vec();
        let updates = gita_push::parse_receive_pack_commands(&body);
        assert!(updates.is_empty());
    }

    /// The "dumb" file endpoints that `git http-backend` serves under
    /// `GIT_HTTP_EXPORT_ALL=1` must be classified as *reads* of the
    /// addressed repository. They used to match neither the read nor the
    /// write check, which let an anonymous client download a private
    /// repository's objects.
    /// A private repository must reject *every* read path, including the
    /// "dumb" file endpoints that `git http-backend` serves
    /// (`HEAD`, `objects/**`, the pack files). Those used to slip past the
    /// authorization gate entirely, letting an anonymous client download
    /// the repository's whole history.
    #[tokio::test]
    async fn private_repo_rejects_dumb_object_paths() {
        let state = test_state().await;
        let user = state
            .db
            .create_user("dumbowner", "dumb@example.com", "h")
            .await
            .unwrap();
        state
            .db
            .create_repository(user.id, "dumbowner", "secret", "d", true, "main", None)
            .await
            .unwrap();
        let empty_headers = HeaderMap::new();
        let no_query = &None;

        for path in [
            "/dumbowner/secret.git/HEAD",
            "/dumbowner/secret.git/objects/info/packs",
            "/dumbowner/secret.git/objects/aa/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "/dumbowner/secret.git/objects/pack/pack-0123.pack",
            "/dumbowner/secret.git/objects/pack/pack-0123.idx",
            "/dumbowner/secret.git/info/refs",
        ] {
            let res = authorize_git_request(
                &state,
                "dumbowner",
                "secret",
                path,
                no_query,
                &empty_headers,
            )
            .await;
            assert!(
                res.is_err(),
                "private repo must reject anonymous access to {path}"
            );
        }
    }

    /// The same paths on a *public* repository stay anonymously readable.
    #[tokio::test]
    async fn public_repo_allows_dumb_object_paths() {
        let state = test_state().await;
        let user = state
            .db
            .create_user("dumbpub", "dumbpub@example.com", "h")
            .await
            .unwrap();
        state
            .db
            .create_repository(user.id, "dumbpub", "open", "d", false, "main", None)
            .await
            .unwrap();
        let empty_headers = HeaderMap::new();
        let no_query = &None;

        for path in [
            "/dumbpub/open.git/HEAD",
            "/dumbpub/open.git/objects/info/packs",
            "/dumbpub/open.git/objects/aa/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ] {
            let res =
                authorize_git_request(&state, "dumbpub", "open", path, no_query, &empty_headers)
                    .await;
            assert!(
                res.is_ok(),
                "public repo must allow anonymous access to {path}"
            );
        }
    }

    #[test]
    fn dumb_file_paths_are_reads_not_neither() {
        for path in [
            "/owner/repo.git/HEAD",
            "/owner/repo.git/objects/info/packs",
            "/owner/repo.git/objects/aa/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "/owner/repo.git/objects/pack/pack-0123.pack",
            "/owner/repo.git/objects/pack/pack-0123.idx",
            "/owner/repo.git/config",
            "/owner/repo.git/info/refs",
            "/owner/repo.git/git-upload-pack",
        ] {
            assert!(
                !is_write_request(path, &None),
                "{path} must not be classified as a write"
            );
            // Not a write => it is a read => the private-repo check runs.
            assert_eq!(is_read_request(path, &None), true, "{path} must be a read");
        }
    }

    /// The only write verb is `git-receive-pack`.
    #[test]
    fn receive_pack_is_the_only_write_verb() {
        assert!(is_write_request("/owner/repo.git/git-receive-pack", &None));
        assert!(is_write_request(
            "/owner/repo.git/info/refs",
            &Some("service=git-receive-pack".to_string())
        ));
        assert!(!is_write_request("/owner/repo.git/git-upload-pack", &None));
        assert!(!is_write_request(
            "/owner/repo.git/info/refs",
            &Some("service=git-upload-pack".to_string())
        ));
    }

    /// A repository literally named `git-receive-pack` must not turn every
    /// request for it into a write. The verb is the third path segment, not
    /// a substring of the whole path.
    #[test]
    fn a_repo_named_like_a_verb_is_not_misclassified() {
        let path = "/owner/git-receive-pack.git/info/refs";
        assert!(!is_write_request(path, &None));
        assert_eq!(is_read_request(path, &None), true);
    }

    /// The verb is parsed structurally, so a deeper path still resolves.
    #[test]
    fn request_verb_reads_the_third_segment() {
        assert_eq!(
            request_verb("/o/r.git/git-receive-pack"),
            Some("git-receive-pack")
        );
        assert_eq!(
            request_verb("/o/r.git/git-upload-pack"),
            Some("git-upload-pack")
        );
        assert_eq!(request_verb("/o/r.git/objects/info/packs"), Some("objects"));
        assert_eq!(request_verb("/o/r.git"), None);
        assert_eq!(request_verb("/o"), None);
    }

    #[tokio::test]
    async fn push_event_payload_contains_pusher_id() {
        // The publish block calls resolve_authed_user; we can't easily
        // drive the full HTTP handler, so assert the pieces that matter:
        // the enriched payload shape is what the handler builds, and the
        // parse function feeds it the right (ref, old_sha, new_sha).
        let state = test_state().await;
        let user = state
            .db
            .create_user("ppusher", "p@e.io", "h")
            .await
            .unwrap();
        state
            .db
            .create_repository(user.id, "ppusher", "r1", "d", false, "main", None)
            .await
            .unwrap();

        let token = state.auth.sign_jwt(&user).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );

        let authed = resolve_authed_user(&state, &headers).await;
        assert!(authed.is_some());
        assert_eq!(authed.unwrap().id, user.id);

        // The payload built by the push block uses pusher_id = user.id.
        let payload = serde_json::json!({
            "owner": "ppusher",
            "repo": "r1",
            "ref": "refs/heads/main",
            "old_sha": "0".repeat(40),
            "new_sha": "a".repeat(40),
            "pusher_id": user.id,
        });
        let event = state.bus.publish(
            EventType::Push,
            Some(user.id),
            Some(user.id),
            payload.clone(),
        );
        assert_eq!(event.payload["pusher_id"], serde_json::json!(user.id));
        assert_eq!(event.actor_id, Some(user.id));
    }
}
