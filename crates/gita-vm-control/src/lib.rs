//! # gita-vm-control
//!
//! gita VM 集群的**控制面**。
//! - 对外提供 REST API (/api/v1/vm/...)
//! - 对内运行协调循环：reconcile、rebalance、node-watch、vm-gc、quota-enforce
//! - 与节点代理通信（polling 模式：节点长轮询 GET /internal/vm/operations/next）

pub mod auth;
pub mod handlers;
pub mod internal;
pub mod loops;
pub mod state;

pub use state::{ControlConfig, AppState, AgentMode};

use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// 构建完整的 HTTP 路由
pub fn build_router(state: AppState) -> Router {
    let api = handlers::router(state.clone())
        .merge(internal::router(state.clone()))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_vm_api_key,
        ));

    Router::new()
        .nest("/api/v1/vm", api)
        .merge(state.metrics.router().with_state(()))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}