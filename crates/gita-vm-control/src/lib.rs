//! # gita-vm-control
//!
//! gita VM 集群的**控制面**。
//! - 对外提供 REST API (/api/v1/vm/...)
//! - 对内运行协调循环：reconcile、rebalance、node-watch、vm-gc、quota-enforce
//! - 与节点代理通信（polling 模式：节点长轮询 GET /internal/vm/operations/next）

pub mod auth;
pub mod ci;
pub mod handlers;
pub mod internal;
pub mod loops;
pub mod state;

pub use state::{AgentMode, AppState, ControlConfig};

use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// 构建完整的 HTTP 路由
pub fn build_router(state: AppState) -> Router {
    // 子路由保持 `Router<AppState>`，这样鉴权中间件才能拿到 state；
    // 整个 /api/v1/vm 前缀共用同一层鉴权。
    let api = handlers::router()
        .merge(ci::ci_routes())
        .merge(internal::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_vm_auth,
        ));

    Router::new()
        .nest("/api/v1/vm", api)
        .merge(state.metrics.router().with_state(()))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}
