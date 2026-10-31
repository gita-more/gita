//! Integration tests for the embedded VM control plane.
//!
//! The VM cluster is a gita-internal feature: when `[server.vm] enabled`
//! is true, gita serves `/api/v1/vm/*` itself and runs the reconcile
//! loops. These tests cover the two states an operator can be in — the
//! feature off (nothing is exposed) and the feature on (the endpoints
//! exist and are authenticated).

mod common;

use axum::http::{Method, StatusCode};
use tower::ServiceExt;

/// Build an app with the VM cluster enabled.
async fn vm_enabled_app() -> (gita_admin::App, common::TestAppGuard) {
    let tmp = tempfile::TempDir::new().unwrap();
    let schema = format!(
        "gita_it_vm_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let base_url = common::test_db_url();
    {
        let pool = sqlx::PgPool::connect(&base_url).await.unwrap();
        sqlx::query(&format!("CREATE SCHEMA \"{schema}\""))
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    let mut config = gita_admin::Config::default();
    config.server.data_dir = tmp.path().join("data");
    config.server.database_url = base_url.clone();
    config.server.database_schema = Some(schema.clone());
    config.server.vm.enabled = true;
    config.server.vm.api_key = Some("test-node-key".into());

    let app = gita_admin::App::build(config).await.unwrap();

    let guard = common::TestAppGuard {
        base_url,
        schema,
        _tmp: tmp,
    };
    (app, guard)
}

/// With the feature off, the VM endpoints must not be served at all.
///
/// A deployment that never enabled the cluster must not expose a
/// half-configured management API.
#[tokio::test]
async fn vm_routes_are_absent_when_the_feature_is_off() {
    let (app, _guard) = common::test_app().await;

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/vm/templates")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();

    // The API catch-all answers unknown /api paths with a JSON 404.
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "VM endpoints must not be registered when the feature is off"
    );
}

/// With the feature on but no token, the endpoints must reject the call.
#[tokio::test]
async fn vm_routes_require_authentication() {
    let (app, _guard) = vm_enabled_app().await;

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/vm/templates")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "VM endpoints must reject unauthenticated callers"
    );
}

/// An authenticated user can list templates; the list starts empty.
#[tokio::test]
async fn authenticated_user_can_list_vm_templates() {
    let (app, _guard) = vm_enabled_app().await;
    let user = common::register_user(&app, "vmuser").await;

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/vm/templates")
        .header("authorization", format!("Bearer {}", user.token))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK, "list failed");
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        v["templates"].as_array().is_some(),
        "response should carry a templates array, got {v}"
    );
    assert_eq!(v["templates"].as_array().unwrap().len(), 0);
}

/// Creating a template is an administrative action: a normal user must be
/// refused.
#[tokio::test]
async fn non_admin_cannot_create_a_vm_template() {
    let (app, _guard) = vm_enabled_app().await;
    let user = common::register_user(&app, "vmplain").await;

    let body = serde_json::json!({
        "name": "ci-runner",
        "spec": {
            "cpu_millis": 2000u64,
            "memory_bytes": 2_000_000_000u64,
            "disk_bytes": 10_000_000_000u64,
            "image": { "name": "ubuntu-24.04" },
            "backend": "qemu",
        },
    });
    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/vm/templates")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", user.token))
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin must not be able to create VM templates"
    );
}

/// An administrator can create a template, and it then shows up in the list.
#[tokio::test]
async fn admin_can_create_a_vm_template() {
    let (app, _guard) = vm_enabled_app().await;
    let admin = common::register_admin(&app, "vmadmin").await;

    let body = serde_json::json!({
        "name": "ci-runner",
        "description": "default CI runner template",
        "spec": {
            "cpu_millis": 2000u64,
            "memory_bytes": 2_000_000_000u64,
            "disk_bytes": 10_000_000_000u64,
            "image": { "name": "ubuntu-24.04" },
            "backend": "qemu",
        },
    });
    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/vm/templates")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", admin.token))
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(created["name"], "ci-runner");
    assert!(
        created["id"].as_i64().unwrap() > 0,
        "the created template must get a database id, got {created}"
    );

    // And it is listed afterwards.
    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/vm/templates")
        .header("authorization", format!("Bearer {}", admin.token))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let names: Vec<&str> = v["templates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"ci-runner"), "got {names:?}");
}

/// The node-facing internal endpoints authenticate with the node API key,
/// not with a user token.
#[tokio::test]
async fn internal_endpoints_require_the_node_api_key() {
    let (app, _guard) = vm_enabled_app().await;

    // Without a key: rejected.
    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/vm/internal/nodes")
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // With the wrong key: rejected.
    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/vm/internal/nodes")
        .header("content-type", "application/json")
        .header("x-api-key", "wrong-key")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // With the configured key: accepted (the body is invalid, so it fails
    // later — but not with 401).
    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/vm/internal/nodes")
        .header("content-type", "application/json")
        .header("x-api-key", "test-node-key")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "the configured node key must be accepted"
    );
}

/// A node that registers with the right key appears in the node list.
#[tokio::test]
async fn node_registration_is_visible_to_admins() {
    let (app, _guard) = vm_enabled_app().await;
    let admin = common::register_admin(&app, "vmnodeadmin").await;

    let body = serde_json::json!({
        "name": "worker-1",
        "cpu_millis": 8000u64,
        "memory_bytes": 16_000_000_000u64,
        "disk_bytes": 100_000_000_000u64,
        "vmm": ["qemu"],
        "arch": ["x86_64"],
        "labels": { "zone": "hz" },
    });
    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/vm/internal/nodes")
        .header("content-type", "application/json")
        .header("x-api-key", "test-node-key")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let registered: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        registered["node_id"].as_i64().unwrap() > 0,
        "the node must get an id, got {registered}"
    );

    // The admin sees it in the node list.
    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/vm/nodes")
        .header("authorization", format!("Bearer {}", admin.token))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let names: Vec<&str> = v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"worker-1"), "got {names:?}");
}
