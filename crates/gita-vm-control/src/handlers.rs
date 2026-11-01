//! 对外 REST API handlers：/api/v1/vm/*

use crate::auth::{ApiAuthError, VmUser};
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use gita_vm_api::ids::OwnerRef;
use gita_vm_api::template::VmTemplate;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// 错误响应
pub struct ApiError(pub StatusCode, pub String);

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

impl From<gita_vm_core::Error> for ApiError {
    fn from(e: gita_vm_core::Error) -> Self {
        use gita_vm_core::Error::*;
        match e {
            NotFound(_) => ApiError(StatusCode::NOT_FOUND, e.to_string()),
            AlreadyExists(_) => ApiError(StatusCode::CONFLICT, e.to_string()),
            InvalidTransition(_) => ApiError(StatusCode::CONFLICT, e.to_string()),
            QuotaExceeded(_) => ApiError(StatusCode::FORBIDDEN, e.to_string()),
            Validation(_) => ApiError(StatusCode::BAD_REQUEST, e.to_string()),
            _ => ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// 把鉴权错误映射成 API 错误（401/403 语义保持不变）
fn auth_err(e: ApiAuthError) -> ApiError {
    match e {
        ApiAuthError::Unauthorized(m) => ApiError(StatusCode::UNAUTHORIZED, m),
        ApiAuthError::Forbidden(m) => ApiError(StatusCode::FORBIDDEN, m),
    }
}

// ===== 请求/响应体 =====

#[derive(Debug, Deserialize)]
pub struct CreateTemplateRequest {
    pub name: String,
    pub spec: gita_vm_api::vm::VmSpec,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub owner: Option<OwnerRef>,
}

#[derive(Debug, Serialize)]
pub struct TemplateResponse {
    pub id: i64,
    pub name: String,
    pub version: u64,
    pub spec: gita_vm_api::vm::VmSpec,
    pub description: String,
    pub owner: OwnerRef,
    pub created_at_ms: i64,
}

impl From<VmTemplate> for TemplateResponse {
    fn from(t: VmTemplate) -> Self {
        Self {
            id: t.id.0,
            name: t.name,
            version: t.version,
            spec: t.spec,
            description: t.description,
            owner: t.owner,
            created_at_ms: t.created_at_ms,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateVmRequest {
    pub template_id: Option<i64>,
    pub spec: Option<gita_vm_api::vm::VmSpec>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub owner: Option<OwnerRef>,
    /// 关联仓库（CI 场景）
    #[serde(default)]
    pub repo_id: Option<i64>,
    /// 过期时间（毫秒时间戳）
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
}

// ===== Handlers =====

async fn healthz() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

async fn list_templates(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let templates = state
        .repo
        .list_templates(None)
        .await
        .map_err(ApiError::from)?;
    let items: Vec<TemplateResponse> = templates.into_iter().map(TemplateResponse::from).collect();
    Ok(Json(json!({ "templates": items })))
}

async fn create_template(
    State(state): State<AppState>,
    user: VmUser,
    Json(req): Json<CreateTemplateRequest>,
) -> ApiResult<impl IntoResponse> {
    // 模板是集群级资源：只有系统管理员能建。
    user.require_admin().map_err(auth_err)?;
    let now = gita_vm_core::now_ms();
    let tmpl = VmTemplate {
        id: gita_vm_api::ids::TemplateId(0),
        name: req.name.clone(),
        version: 1,
        spec: req.spec.clone(),
        image_id: None,
        kernel_image_id: None,
        owner: req.owner.clone().unwrap_or_else(OwnerRef::system),
        tags: vec![],
        description: req.description.clone(),
        created_at_ms: now,
    };

    state.repo.create_template(&tmpl).await.map_err(ApiError::from)?;

    // 取回分配的 ID
    let templates = state.repo.list_templates(None).await.map_err(ApiError::from)?;
    let created = templates
        .into_iter()
        .find(|t| t.name == req.name)
        .ok_or_else(|| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "template not found after create".into()))?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(TemplateResponse::from(created)).unwrap()),
    ))
}

async fn get_template(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let tmpl = state
        .repo
        .get_template(gita_vm_api::ids::TemplateId(id))
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("template {id} not found")))?;
    Ok(Json(serde_json::to_value(TemplateResponse::from(tmpl)).unwrap()))
}

async fn delete_template(
    State(state): State<AppState>,
    user: VmUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    user.require_admin().map_err(auth_err)?;
    state
        .repo
        .delete_template(gita_vm_api::ids::TemplateId(id))
        .await
        .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_vms(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let vms = state.repo.list_vms(None).await.map_err(ApiError::from)?;
    Ok(Json(json!({ "vms": vms })))
}

async fn get_vm(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let vm = state
        .repo
        .get_vm(gita_vm_api::ids::VmId(id))
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("vm {id} not found")))?;
    Ok(Json(serde_json::to_value(vm).unwrap()))
}

async fn delete_vm(
    State(state): State<AppState>,
    user: VmUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    user.require_admin().map_err(auth_err)?;
    let vm_id = gita_vm_api::ids::VmId(id);
    let vm = state
        .repo
        .get_vm(vm_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("vm {id} not found")))?;

    if !gita_vm_core::state::can_delete(vm.status) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!("vm {id} is in {:?} state, cannot delete", vm.status),
        ));
    }

    state.repo.delete_vm(vm_id).await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_nodes(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let nodes = state.repo.list_nodes(None).await.map_err(ApiError::from)?;
    Ok(Json(json!({ "nodes": nodes })))
}

/// GET /api-key
///
/// 管理员专用：读出节点 API key。key 由部署端在启动时解析/生成并
/// 持久化（[`crate::internal::resolve_node_api_key`]），这里只读取，
/// 不临时随机——保证两次查询结果一致，`gita-admin vm api-key` 把它
/// 交到节点代理的 `--api-key` 手里。
async fn get_api_key(
    State(state): State<AppState>,
    user: VmUser,
) -> ApiResult<Json<serde_json::Value>> {
    user.require_admin().map_err(auth_err)?;
    let Some(key) = state.auth.node_api_key() else {
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "node api key not initialised".into(),
        ));
    };
    Ok(Json(json!({ "api_key": key })))
}

async fn get_node(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let node = state
        .repo
        .get_node(gita_vm_api::ids::NodeId(id))
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("node {id} not found")))?;
    Ok(Json(serde_json::to_value(node).unwrap()))
}

async fn list_images(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let images = state.repo.list_images(None).await.map_err(ApiError::from)?;
    Ok(Json(json!({ "images": images })))
}

async fn list_quotas(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let quotas = state.repo.list_quotas().await.map_err(ApiError::from)?;
    Ok(Json(json!({ "quotas": quotas })))
}

/// 路由表
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        // templates
        .route("/templates", get(list_templates).post(create_template))
        .route("/templates/{id}", get(get_template).delete(delete_template))
        // vms
        .route("/vms", get(list_vms))
        .route("/vms/{id}", get(get_vm).delete(delete_vm))
        // nodes
        .route("/nodes", get(list_nodes))
        .route("/nodes/{id}", get(get_node))
        .route("/api-key", get(get_api_key))
        // images
        .route("/images", get(list_images))
        // quotas
        .route("/quotas", get(list_quotas))
}
