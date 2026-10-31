//! 认证中间件：复用 gita 的 JWT 鉴权，VM API 需要 admin 权限
//! （节点内部接口使用 API key）

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use crate::state::AppState;

/// VM API 鉴权中间件
///
/// - `/internal/*` 路径：节点代理使用 API key（`X-Api-Key` 头）
/// - 其他路径：用户 JWT，需要 `is_admin` 或对应资源权限
pub async fn require_vm_api_key(
    State(_state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    // TODO: 与 gita-http 的 JWT 鉴权打通；当前阶段先做基本存在性校验
    let path = req.uri().path();

    // 健康检查豁免
    if path.ends_with("/healthz") {
        return next.run(req).await;
    }

    // 内部接口校验 API key
    if path.contains("/internal/") {
        let has_key = req
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if !has_key {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "missing api key"})),
            )
                .into_response();
        }
        return next.run(req).await;
    }

    // 其他接口：TODO 校验 JWT（集成阶段与 gita-api 的 auth 中间件统一）
    next.run(req).await
}
