//! VM API 鉴权
//!
//! 两套身份：
//!
//! - **用户身份**：复用 gita 的 JWT（`gita_api::auth::Auth`）。令牌被吊销
//!   后立刻失效——这一点必须与 gita 主 API 完全一致，否则一个被吊销的
//!   管理员令牌仍能管理 VM 集群。
//! - **节点身份**：节点代理访问 `/internal/*` 接口。新节点用 enrollment
//!   token 换取自己的 `node_id` + secret，之后请求带 `X-Node-Secret`；
//!   旧的 `X-Api-Key` 共享密钥仍作为后备接受。节点只需要出站连接，因此
//!   不需要 gita 用户身份。

use crate::state::AppState;
use axum::{
    extract::{FromRequestParts, Request, State},
    http::{request::Parts, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use gita_api::auth::Auth;
use gita_core::User;
use gita_db::Db;
use serde_json::json;
use std::sync::Arc;

/// VM 鉴权上下文
#[derive(Clone)]
pub struct VmAuth {
    /// gita 的 JWT 校验器（含吊销列表检查）
    auth: Arc<Auth>,
    /// 数据库（查用户、查吊销列表）
    db: Db,
    /// 节点代理使用的 API key。
    ///
    /// `None` 表示服务端没有配置 key，此时内部接口一律拒绝——调用方
    /// （gita-admin 与独立 gita-vm-control）都会在启动时经
    /// [`crate::internal::resolve_node_api_key`] 解析出 key：显式配置的
    /// 优先，否则生成随机值并持久化。因此运行中的控制面总持有 key。
    node_api_key: Option<String>,
}

impl VmAuth {
    pub fn new(db: Db, auth: Arc<Auth>, node_api_key: Option<String>) -> Self {
        Self {
            auth,
            db,
            node_api_key: node_api_key.filter(|k| !k.is_empty()),
        }
    }

    /// 当前生效的节点 API key（`resolve_node_api_key` 之后才有值）。
    pub fn node_api_key(&self) -> Option<&str> {
        self.node_api_key.as_deref()
    }

    /// 从 Bearer 令牌解析用户身份。
    ///
    /// 令牌以**所有权**传入：`&Request` 不是 `Send`（`Request<Body>` 不
    /// 是 `Sync`），在 `.await` 期间持有它会让中间件的 future 变成
    /// 非 `Send`，axum 的 `from_fn_with_state` 随即拒绝这个中间件。
    async fn resolve_user(&self, token: &str) -> Result<User, ApiAuthError> {
        self.auth
            .resolve_bearer(&self.db, token)
            .await
            .map_err(|_| ApiAuthError::Unauthorized("invalid or revoked token".into()))
    }

    /// 数据库句柄。
    ///
    /// 控制面自己要写 gita 侧的表（CI job 的 agent token），而不只是读
    /// 用户和吊销列表。
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// 校验节点身份：显式的全局 API key，或该节点自己的 secret。
    ///
    /// 全局 key 是旧部署的兼容路径；新部署走 enrollment，节点只持有
    /// 自己的 secret，泄露一个节点不会影响其它节点。
    pub async fn check_node_access(
        &self,
        node_id: Option<i64>,
        api_key: Option<&str>,
        node_secret: Option<&str>,
    ) -> Result<(), ApiAuthError> {
        if let (Some(expected), Some(presented)) = (self.node_api_key.as_deref(), api_key) {
            if !presented.is_empty() && presented == expected {
                return Ok(());
            }
        }

        if let (Some(node_id), Some(secret)) = (node_id, node_secret) {
            if !secret.is_empty() {
                let hash = gita_db::hash_token(secret);
                match self.db.node_secret_matches(node_id, &hash).await {
                    Ok(true) => return Ok(()),
                    Ok(false) => {}
                    Err(e) => return Err(ApiAuthError::Internal(e.to_string())),
                }
            }
        }

        Err(ApiAuthError::Unauthorized(
            "missing or invalid node credentials".into(),
        ))
    }
}

/// 从请求头取出 Bearer 令牌。
///
/// 只认 `Authorization: Bearer <token>`（大小写不敏感的前缀由调用方保证，
/// 这里按标准写法匹配）。`Basic` 或其它 scheme 一律视为没有令牌。
pub fn bearer_token(req: &Request) -> Option<&str> {
    let raw = req.headers().get("authorization")?.to_str().ok()?;
    let (scheme, token) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// 从请求头取出节点 API key。
pub fn api_key(req: &Request) -> Option<&str> {
    let raw = req.headers().get("x-api-key")?.to_str().ok()?.trim();
    if raw.is_empty() {
        None
    } else {
        Some(raw)
    }
}

/// 从请求头取出节点自己的 secret。
pub fn node_secret(req: &Request) -> Option<&str> {
    let raw = req.headers().get("x-node-secret")?.to_str().ok()?.trim();
    if raw.is_empty() {
        None
    } else {
        Some(raw)
    }
}

/// 从 `/internal/nodes/{id}/...` 里取出节点 id。
///
/// 识别不了就返回 `None`；调用方随后只接受全局 key，因此一个拼错的
/// 路径不会意外绕过节点身份检查。
pub fn internal_node_id(path: &str) -> Option<i64> {
    let mut segments = path.split('/');
    while let Some(segment) = segments.next() {
        if segment == "nodes" {
            return segments.next().and_then(|id| id.parse::<i64>().ok());
        }
    }
    None
}

/// 鉴权失败
#[derive(Debug)]
pub enum ApiAuthError {
    Unauthorized(String),
    Forbidden(String),
    Internal(String),
}

impl IntoResponse for ApiAuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiAuthError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
            ApiAuthError::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            ApiAuthError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

/// 鉴权中间件
///
/// - `/healthz` 豁免
/// - `/internal/*` 走节点 API key
/// - 其余路径走用户 JWT，并把解析出的用户放进扩展里供 handler 取用
///
/// 控制面**始终**带鉴权运行，没有"跳过鉴权"的开关。
pub async fn require_vm_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();

    if path.ends_with("/healthz") {
        return next.run(req).await;
    }

    let auth = state.auth.clone();

    if path.contains("/internal/") {
        // Enrollment is authenticated by the join token in the request
        // header, which the handler redeems; there is no node identity
        // to check yet.
        if path.ends_with("/internal/nodes/enroll") {
            return next.run(req).await;
        }

        // 取成所有权再校验：借用 request 的引用跨不过 await。
        let key = api_key(&req).map(str::to_string);
        let secret = node_secret(&req).map(str::to_string);
        let node_id = internal_node_id(&path);
        if let Err(e) = auth
            .check_node_access(node_id, key.as_deref(), secret.as_deref())
            .await
        {
            return e.into_response();
        }
        return next.run(req).await;
    }

    let token = match bearer_token(&req) {
        Some(t) => t.to_string(),
        None => return ApiAuthError::Unauthorized("missing bearer token".into()).into_response(),
    };

    match auth.resolve_user(&token).await {
        Ok(user) => {
            let (mut parts, body) = req.into_parts();
            parts.extensions.insert(user);
            next.run(Request::from_parts(parts, body)).await
        }
        Err(e) => e.into_response(),
    }
}

/// 已认证用户提取器
#[derive(Debug, Clone)]
pub struct VmUser(pub User);

impl VmUser {
    /// 是否系统管理员
    pub fn is_admin(&self) -> bool {
        self.0.is_admin
    }

    /// 要求系统管理员权限
    pub fn require_admin(&self) -> Result<(), ApiAuthError> {
        if self.0.is_admin {
            Ok(())
        } else {
            Err(ApiAuthError::Forbidden(
                "admin privileges required for VM management".into(),
            ))
        }
    }
}

impl<S> FromRequestParts<S> for VmUser
where
    S: Send + Sync,
{
    type Rejection = ApiAuthError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<User>()
            .cloned()
            .map(VmUser)
            .ok_or_else(|| ApiAuthError::Unauthorized("not authenticated".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with_header(name: &str, value: &str) -> Request {
        Request::builder()
            .uri("/api/v1/vm/templates")
            .header(name, value)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    #[test]
    fn bearer_token_is_extracted() {
        let req = request_with_header("authorization", "Bearer abc123");
        assert_eq!(bearer_token(&req), Some("abc123"));
    }

    #[test]
    fn bearer_token_is_trimmed() {
        let req = request_with_header("authorization", "Bearer   abc123  ");
        assert_eq!(bearer_token(&req), Some("abc123"));
    }

    /// 非 Bearer 的 scheme 不算数——否则 `Basic xxx` 会被当成令牌去校验。
    #[test]
    fn non_bearer_scheme_is_not_a_token() {
        let req = request_with_header("authorization", "Basic dXNlcjpwYXNz");
        assert_eq!(bearer_token(&req), None);
    }

    #[test]
    fn empty_bearer_token_is_rejected() {
        let req = request_with_header("authorization", "Bearer   ");
        assert_eq!(bearer_token(&req), None);
    }

    #[test]
    fn missing_authorization_header_is_none() {
        let req = request_with_header("x-other", "value");
        assert_eq!(bearer_token(&req), None);
    }

    #[test]
    fn api_key_is_extracted_and_trimmed() {
        let req = request_with_header("x-api-key", "  key-123  ");
        assert_eq!(api_key(&req), Some("key-123"));
    }

    #[test]
    fn empty_api_key_is_none() {
        let req = request_with_header("x-api-key", "   ");
        assert_eq!(api_key(&req), None);
    }

    #[test]
    fn node_secret_is_extracted_and_trimmed() {
        let req = request_with_header("x-node-secret", "  secret-1  ");
        assert_eq!(node_secret(&req), Some("secret-1"));
    }

    #[test]
    fn empty_node_secret_is_none() {
        let req = request_with_header("x-node-secret", "   ");
        assert_eq!(node_secret(&req), None);
    }

    #[test]
    fn the_node_id_is_read_from_the_internal_path() {
        assert_eq!(
            internal_node_id("/api/v1/vm/internal/nodes/42/heartbeat"),
            Some(42)
        );
        assert_eq!(
            internal_node_id("/api/v1/vm/internal/nodes/7/operations/next"),
            Some(7)
        );
    }

    #[test]
    fn a_path_without_a_node_id_yields_none() {
        assert_eq!(internal_node_id("/api/v1/vm/internal/nodes"), None);
        assert_eq!(internal_node_id("/api/v1/vm/internal/nodes/enroll"), None);
        assert_eq!(internal_node_id("/api/v1/vm/internal/nodes/abc/heartbeat"), None);
    }
}
