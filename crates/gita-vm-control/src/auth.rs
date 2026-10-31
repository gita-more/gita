//! VM API 鉴权
//!
//! 两套身份：
//!
//! - **用户身份**：复用 gita 的 JWT（`gita_api::auth::Auth`）。令牌被吊销
//!   后立刻失效——这一点必须与 gita 主 API 完全一致，否则一个被吊销的
//!   管理员令牌仍能管理 VM 集群。
//! - **节点身份**：节点代理用 `X-Api-Key` 头访问 `/internal/*` 接口。
//!   节点只需要出站连接，因此不需要 gita 用户身份。

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
    /// 节点代理使用的 API key；`None` 表示未配置（此时内部接口只要求
    /// 存在非空 key，方便本地开发，生产必须配置）
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

    /// 校验节点 API key
    fn check_node_key(&self, presented: &str) -> Result<(), ApiAuthError> {
        if presented.is_empty() {
            return Err(ApiAuthError::Unauthorized("missing api key".into()));
        }
        match &self.node_api_key {
            // 配置了 key 就必须匹配；没配置只要求非空（开发模式）
            Some(expected) if presented != expected => {
                Err(ApiAuthError::Unauthorized("invalid api key".into()))
            }
            _ => Ok(()),
        }
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

/// 鉴权失败
#[derive(Debug)]
pub enum ApiAuthError {
    Unauthorized(String),
    Forbidden(String),
}

impl IntoResponse for ApiAuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiAuthError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
            ApiAuthError::Forbidden(m) => (StatusCode::FORBIDDEN, m),
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
        // 取成所有权再校验：借用 request 的引用跨不过 await。
        let key = api_key(&req).unwrap_or("").to_string();
        if let Err(e) = auth.check_node_key(&key) {
            return e.into_response();
        }
        return next.run(req).await;
    }

    let token = match bearer_token(&req) {
        Some(t) => t.to_string(),
        None => {
            return ApiAuthError::Unauthorized("missing bearer token".into()).into_response()
        }
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
}
