//! 内部接口：节点代理专用（长轮询拉取操作、上报状态）

use rand::RngCore;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use gita_vm_api::node::{Node, NodePhase, NodeStatus};
use serde::Deserialize;
use serde_json::json;

/// 解析节点 API key：显式配置的值优先；否则取持久化值；没有就生成
/// 一个随机值写进 `vm_settings` 后返回。并发首启时先写者赢，输的
/// 一方读回赢家的值（[`gita_db::vm_settings::Db::insert_setting`]）。
///
/// 部署不配 key（历史上是“任意非空都能过”的开发模式）会在这里第一次
/// 启动时被替代成“服务端生成的 key 并持久化”，之后每次启动一致。
pub async fn resolve_node_api_key(
    db: &gita_db::Db,
    configured: Option<String>,
) -> anyhow::Result<String> {
    if let Some(key) = configured.filter(|k| !k.trim().is_empty()) {
        return Ok(key);
    }
    if let Some(key) = db.get_setting(gita_db::vm_settings::NODE_API_KEY).await? {
        return Ok(key);
    }
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let key = hex::encode(bytes);
    let inserted = db
        .insert_setting(
            gita_db::vm_settings::NODE_API_KEY,
            &key,
            gita_vm_core::now_ms(),
        )
        .await?;
    if inserted {
        return Ok(key);
    }
    // 并发首启：另一个控制面抢先写入了它的 key，以它为准。
    db.get_setting(gita_db::vm_settings::NODE_API_KEY)
        .await?
        .ok_or_else(|| anyhow::anyhow!("node api key missing after insert conflict"))
}

/// 节点注册请求
#[derive(Debug, Deserialize)]
pub struct NodeRegisterRequest {
    pub name: String,
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub vmm: Vec<String>,
    pub arch: Vec<String>,
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
}

fn internal_error(message: impl std::fmt::Display) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": message.to_string() })),
    )
}

/// Create the node row and return it with the id the database assigned.
async fn create_node(
    state: &AppState,
    req: &NodeRegisterRequest,
) -> Result<Node, (StatusCode, Json<serde_json::Value>)> {
    use gita_vm_api::node::{NodeCapabilities, NodeSpec};

    let now = gita_vm_core::now_ms();
    let vmm: Vec<gita_vm_api::node::VmmKind> = req
        .vmm
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect();

    let node = Node {
        id: gita_vm_api::ids::NodeId(0),
        name: req.name.clone(),
        cell: None,
        spec: NodeSpec {
            cpu_millis: req.cpu_millis,
            memory_bytes: req.memory_bytes,
            disk_bytes: req.disk_bytes,
            capabilities: NodeCapabilities {
                vmm,
                arch: req.arch.clone(),
                nested_virt: false,
                gpu_passthrough: false,
                max_vcpus: 64,
                max_memory_bytes: req.memory_bytes,
            },
        },
        status: NodeStatus {
            alloc_cpu_millis: 0,
            alloc_memory_bytes: 0,
            alloc_disk_bytes: 0,
            running_vms: 0,
            phase: NodePhase::Joining,
            last_heartbeat_ms: now,
        },
        taints: vec![],
        labels: req.labels.clone(),
        owner: gita_vm_api::ids::OwnerRef::system(),
        conditions: vec![],
        created_at_ms: now,
        updated_at_ms: now,
    };

    state.repo.create_node(&node).await.map_err(|e| {
        let message = e.to_string();
        if message.contains("duplicate key") || message.contains("unique") {
            (
                StatusCode::CONFLICT,
                Json(json!({ "error": "a node with that name already exists" })),
            )
        } else {
            internal_error(message)
        }
    })?;

    // 查回分配的 ID
    let nodes = state.repo.list_nodes(None).await.map_err(internal_error)?;
    nodes
        .into_iter()
        .find(|n| n.name == req.name)
        .ok_or_else(|| internal_error("node not found after create"))
}

/// 节点注册（旧路径：共享 API key）
async fn register_node(
    State(state): State<AppState>,
    Json(req): Json<NodeRegisterRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let created = create_node(&state, &req).await?;
    Ok(Json(json!({
        "node_id": created.id.0,
        "poll_interval_secs": 10,
    })))
}

fn random_secret(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("{prefix}{}", hex::encode(bytes))
}

/// 节点加入（新路径：一次性 join token 换取每节点 secret）
///
/// `X-Join-Token` 携带管理员在页面上生成的一次性 token。成功后控制面
/// 创建节点、写入该节点 secret 的哈希、标记 token 已使用，并把明文
/// secret 返回一次——节点随后只用自己的 secret 认证。
async fn enroll_node(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<NodeRegisterRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let token = headers
        .get("x-join-token")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "missing join token" })),
            )
        })?
        .to_string();

    let db = state.auth.db();
    let now = gita_vm_core::now_ms();
    let token_hash = gita_db::hash_token(&token);
    let enrollment = db
        .find_valid_enrollment_token(&token_hash, now)
        .await
        .map_err(internal_error)?
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "invalid, used or expired join token" })),
            )
        })?;

    if let Some(bound) = enrollment.node_name.as_deref() {
        if bound != req.name {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "join token is bound to another node name" })),
            ));
        }
    }

    let created = create_node(&state, &req).await?;
    let node_id = created.id.0;
    let secret = random_secret("gita_node_");
    db.set_node_secret(node_id, &gita_db::hash_token(&secret), now)
        .await
        .map_err(internal_error)?;

    // The claim is the linearization point: two agents racing with the
    // same token must not both end up with a credential.
    let claimed = db
        .consume_enrollment_token(&token_hash, now, node_id)
        .await
        .map_err(internal_error)?;
    if !claimed {
        let _ = state
            .repo
            .delete_node(gita_vm_api::ids::NodeId(node_id))
            .await;
        return Err((
            StatusCode::CONFLICT,
            Json(json!({ "error": "join token was already used" })),
        ));
    }

    tracing::info!(node_id, name = %created.name, "node enrolled");
    Ok(Json(json!({
        "node_id": node_id,
        "node_secret": secret,
        "poll_interval_secs": 10,
    })))
}

/// 节点心跳
async fn node_heartbeat(
    State(state): State<AppState>,
    Path(node_id): Path<i64>,
    Json(status): Json<NodeStatus>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let id = gita_vm_api::ids::NodeId(node_id);
    let mut node = state
        .repo
        .get_node(id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("node {node_id} not found")})),
            )
        })?;

    node.status.alloc_cpu_millis = status.alloc_cpu_millis;
    node.status.alloc_memory_bytes = status.alloc_memory_bytes;
    node.status.alloc_disk_bytes = status.alloc_disk_bytes;
    node.status.running_vms = status.running_vms;
    node.status.last_heartbeat_ms = gita_vm_core::now_ms();
    if !matches!(node.status.phase, NodePhase::Draining | NodePhase::Maintenance) {
        node.status.phase = NodePhase::Online;
    }

    state
        .repo
        .update_node(&node)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

    Ok(Json(json!({"ok": true})))
}

/// 长轮询拉取待执行操作（Polling 模式核心）
async fn next_operations(
    State(state): State<AppState>,
    Path(node_id): Path<i64>,
) -> Json<serde_json::Value> {
    let nid = gita_vm_api::ids::NodeId(node_id);
    let ops = state
        .repo
        .list_pending_operations(Some(nid), 10)
        .await
        .unwrap_or_default();
    Json(json!({ "operations": ops }))
}

/// 上报操作执行结果
#[derive(Debug, Deserialize)]
pub struct OperationReport {
    pub operation_id: i64,
    pub success: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub output: serde_json::Value,
}

async fn report_operation(
    State(state): State<AppState>,
    Path(_node_id): Path<i64>,
    Json(report): Json<OperationReport>,
) -> Json<serde_json::Value> {
    let op_id = gita_vm_api::ids::OperationId(report.operation_id);
    if let Ok(Some(mut op)) = state.repo.get_operation(op_id).await {
        if report.success {
            op.mark_succeeded();
        } else {
            op.mark_failed(report.error.unwrap_or_else(|| "node reported failure".into()));
        }
        let _ = state.repo.update_operation(&op).await;
    }
    Json(json!({"ok": true}))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/internal/nodes", post(register_node))
        .route("/internal/nodes/enroll", post(enroll_node))
        .route("/internal/nodes/{id}/heartbeat", post(node_heartbeat))
        .route("/internal/nodes/{id}/operations/next", get(next_operations))
        .route("/internal/nodes/{id}/operations/report", post(report_operation))
}
