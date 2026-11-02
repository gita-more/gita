//! The VM cluster's API, end to end.
//!
//! `vm_api.rs` covers who may reach the VM endpoints. This file covers
//! what the endpoints do once they are reached: a node registers and
//! heartbeats, a template is created and read back, and the quotas that
//! limit the cluster are listed.

mod common;

use axum::http::{Method, StatusCode};
use tower::ServiceExt;

/// Build an app with the VM cluster enabled.
async fn vm_enabled_app() -> (gita_admin::App, common::TestAppGuard) {
    common::test_app_with_vm().await
}

/// Issue a request against the app and return the status and the body.
async fn call(
    app: &gita_admin::App,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
    token: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = axum::http::Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => builder
            .header("content-type", "application/json")
            .body(axum::body::Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap(),
        None => builder.body(axum::body::Body::empty()).unwrap(),
    };
    let resp = app.router().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, value)
}

/// Register a node with the node API key.
async fn register_node(app: &gita_admin::App, name: &str) -> serde_json::Value {
    let body = serde_json::json!({
        "name": name,
        "cpu_millis": 8000u64,
        "memory_bytes": 16_000_000_000u64,
        "disk_bytes": 100_000_000_000u64,
        "vmm": ["qemu"],
        "arch": ["x86_64"],
        "labels": { "zone": "hz" },
    });
    let builder = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/vm/internal/nodes")
        .header("content-type", "application/json")
        .header("x-api-key", "test-vm-key");
    let req = builder
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "the node must register");
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// A node registers, heartbeats, and is then visible to an admin.
#[tokio::test]
async fn a_node_registers_heartbeats_and_is_visible() {
    let (app, _guard) = vm_enabled_app().await;
    let admin = common::register_admin(&app, "vmflowadmin").await;

    let registered = register_node(&app, "worker-1").await;
    let node_id = registered["node_id"].as_i64().expect("the node must get an id");
    assert!(node_id > 0, "got {registered}");

    // A heartbeat without the node key is refused.
    let (status, body) = call(
        &app,
        Method::POST,
        &format!("/api/v1/vm/internal/nodes/{node_id}/heartbeat"),
        Some(serde_json::json!({})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "got {body}");

    // A heartbeat moves the node online and records what it reports.
    let heartbeat = serde_json::json!({
        "alloc_cpu_millis": 1000i64,
        "alloc_memory_bytes": 1024i64,
        "alloc_disk_bytes": 2048i64,
        "running_vms": 1i32,
        "phase": "online",
        "last_heartbeat_ms": 0i64,
    });
    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/vm/internal/nodes/{node_id}/heartbeat"))
        .header("content-type", "application/json")
        .header("x-api-key", "test-vm-key")
        .body(axum::body::Body::from(
            serde_json::to_vec(&heartbeat).unwrap(),
        ))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "the heartbeat must be accepted");

    // The admin sees the node and its phase.
    let (status, body) = call(
        &app,
        Method::GET,
        "/api/v1/vm/nodes",
        None,
        Some(&admin.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got {body}");
    let nodes = body["nodes"].as_array().expect("the node list");
    let worker = nodes
        .iter()
        .find(|n| n["name"] == "worker-1")
        .expect("the node must be listed");
    assert_eq!(worker["id"].as_i64(), Some(node_id));
    assert_eq!(worker["status"]["phase"], "online");
    assert_eq!(worker["status"]["running_vms"], 1);
}

/// A template an admin creates is listed, readable, and deletable.
#[tokio::test]
async fn a_template_is_created_read_and_deleted() {
    let (app, _guard) = vm_enabled_app().await;
    let admin = common::register_admin(&app, "vmtmpladmin").await;

    let spec = serde_json::json!({
        "cpu_millis": 1000u64,
        "memory_bytes": 1024u64 * 1024u64,
        "disk_bytes": 2048u64,
        "image": { "name": "ubuntu-24.04" },
        "backend": "qemu",
    });
    let (status, body) = call(
        &app,
        Method::POST,
        "/api/v1/vm/templates",
        Some(serde_json::json!({
            "name": "ubuntu",
            "spec": spec,
            "description": "the default template",
        })),
        Some(&admin.token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "got {body}");
    let template_id = body["id"].as_i64().expect("the template must get an id");
    assert_eq!(body["name"], "ubuntu");
    assert_eq!(body["spec"]["cpu_millis"], 1000);

    // The template is listed.
    let (status, body) = call(
        &app,
        Method::GET,
        "/api/v1/vm/templates",
        None,
        Some(&admin.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got {body}");
    let names: Vec<&str> = body["templates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"ubuntu"), "got {names:?}");

    // It can be read on its own.
    let (status, body) = call(
        &app,
        Method::GET,
        &format!("/api/v1/vm/templates/{template_id}"),
        None,
        Some(&admin.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got {body}");
    assert_eq!(body["description"], "the default template");

    // And deleted.
    let (status, body) = call(
        &app,
        Method::DELETE,
        &format!("/api/v1/vm/templates/{template_id}"),
        None,
        Some(&admin.token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "got {body}");

    let (status, _) = call(
        &app,
        Method::GET,
        &format!("/api/v1/vm/templates/{template_id}"),
        None,
        Some(&admin.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A template that is not there is a 404, not an empty answer.
#[tokio::test]
async fn an_unknown_template_is_not_found() {
    let (app, _guard) = vm_enabled_app().await;
    let admin = common::register_admin(&app, "vmtmpl404").await;
    let (status, _) = call(
        &app,
        Method::GET,
        "/api/v1/vm/templates/9999",
        None,
        Some(&admin.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The quotas of the cluster are listed, and the list is empty until
/// one is set.
#[tokio::test]
async fn the_quotas_are_listed() {
    let (app, _guard) = vm_enabled_app().await;
    let admin = common::register_admin(&app, "vmquotaadmin").await;

    let (status, body) = call(&app, Method::GET, "/api/v1/vm/quotas", None, Some(&admin.token)).await;
    assert_eq!(status, StatusCode::OK, "got {body}");
    assert!(
        body["quotas"].as_array().unwrap().is_empty(),
        "a fresh cluster has no quotas, got {body}"
    );

    // Set one and see it come back.
    sqlx::query(
        "INSERT INTO vm_quotas (scope, scope_id, max_concurrent_vms, max_cpu_millis,
                                max_memory_bytes, max_disk_bytes, max_vm_lifetime_ms,
                                created_at_ms, updated_at_ms)
         VALUES ('system', 0, 10, 40000, 64000000000, 200000000000, 0,
                 EXTRACT(EPOCH FROM NOW()) * 1000, EXTRACT(EPOCH FROM NOW()) * 1000)",
    )
    .execute(app.db.pool())
    .await
    .unwrap();

    let (status, body) = call(&app, Method::GET, "/api/v1/vm/quotas", None, Some(&admin.token)).await;
    assert_eq!(status, StatusCode::OK, "got {body}");
    let quotas = body["quotas"].as_array().unwrap();
    assert_eq!(quotas.len(), 1, "got {body}");
    assert_eq!(quotas[0]["scope"], "system");
    assert_eq!(quotas[0]["max_concurrent_vms"], 10);
}

/// The quotas of the cluster are an administrator's view.
///
/// They are the limits every tenant lives under — how many VMs, how
/// much CPU, how much memory — so reading them tells a user about the
/// other tenants' allowances. A user who wants to know what they may
/// still create is told when the VM is created, not here.
#[tokio::test]
async fn the_quotas_are_for_the_administrators() {
    let (app, _guard) = vm_enabled_app().await;
    let user = common::register_user(&app, "vmquotauser").await;

    let (status, body) = call(
        &app,
        Method::GET,
        "/api/v1/vm/quotas",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "got {body}");

    // And nobody at all is turned away at the door.
    let (status, _) = call(&app, Method::GET, "/api/v1/vm/quotas", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// The instances of the cluster are an administrator's view too.
///
/// They carry their names and their addresses, and they belong to the
/// tenants that made them.
#[tokio::test]
async fn the_instances_are_for_the_administrators() {
    let (app, _guard) = vm_enabled_app().await;
    let user = common::register_user(&app, "vmvmuser").await;

    for uri in [
        "/api/v1/vm/vms",
        "/api/v1/vm/vms/1",
        "/api/v1/vm/nodes",
        "/api/v1/vm/nodes/1",
        "/api/v1/vm/images",
    ] {
        let (status, body) = call(&app, Method::GET, uri, None, Some(&user.token)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} must be admin-only, got {body}");
    }
}

/// The node API key is only for the node endpoints: a user token must
/// not open them, and the node key must not open the admin endpoints.
#[tokio::test]
async fn the_node_key_and_the_user_token_do_not_open_each_others_endpoints() {
    let (app, _guard) = vm_enabled_app().await;
    let admin = common::register_admin(&app, "vmkeyadmin").await;

    // A user token does not open the node endpoints.
    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/vm/internal/nodes")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", admin.token))
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a user token must not open the node endpoints"
    );

    // The node key does not open the admin endpoints.
    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/vm/nodes")
        .header("x-api-key", "test-vm-key")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "the node key must not open the admin endpoints"
    );
}
