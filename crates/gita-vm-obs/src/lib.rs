//! # gita-vm-obs
//!
//! VM 观测指标：Prometheus metrics

use axum::Router;
use lazy_static::lazy_static;
use prometheus::{Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGaugeVec, Registry, TextEncoder};
use std::sync::Arc;

lazy_static! {
    // ===== VM 指标 =====
    pub static ref METRIC_VMS_TOTAL: IntGaugeVec = IntGaugeVec::new(
        prometheus::Opts::new("gita_vm_vms_total", "Total VMs by status"),
        &["status"],
    ).unwrap();

    pub static ref METRIC_VM_CREATED_TOTAL: IntCounterVec = IntCounterVec::new(
        prometheus::Opts::new("gita_vm_vm_created_total", "Total VMs created"),
        &["template"],
    ).unwrap();

    pub static ref METRIC_VM_DELETED_TOTAL: IntCounterVec = IntCounterVec::new(
        prometheus::Opts::new("gita_vm_vm_deleted_total", "Total VMs deleted"),
        &["reason"],
    ).unwrap();

    // ===== 节点指标 =====
    pub static ref METRIC_NODES_TOTAL: IntGaugeVec = IntGaugeVec::new(
        prometheus::Opts::new("gita_vm_nodes_total", "Total nodes by phase"),
        &["phase"],
    ).unwrap();

    pub static ref METRIC_NODE_CPU_USAGE: IntGaugeVec = IntGaugeVec::new(
        prometheus::Opts::new("gita_vm_node_cpu_millis", "Node allocated CPU millicores"),
        &["node"],
    ).unwrap();

    pub static ref METRIC_NODE_MEMORY_USAGE: IntGaugeVec = IntGaugeVec::new(
        prometheus::Opts::new("gita_vm_node_memory_bytes", "Node allocated memory bytes"),
        &["node"],
    ).unwrap();

    // ===== 操作指标 =====
    pub static ref METRIC_OPERATIONS_TOTAL: IntCounterVec = IntCounterVec::new(
        prometheus::Opts::new("gita_vm_operations_total", "Total operations"),
        &["kind", "status"],
    ).unwrap();

    pub static ref METRIC_OPERATION_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("gita_vm_operation_duration_seconds", "Operation duration"),
        &["kind"],
    ).unwrap();

    // ===== IPAM 指标 =====
    pub static ref METRIC_IP_LEASES_TOTAL: IntGaugeVec = IntGaugeVec::new(
        prometheus::Opts::new("gita_vm_ip_leases_total", "Active IP leases"),
        &["network", "state"],
    ).unwrap();

    // ===== 事件指标 =====
    pub static ref METRIC_EVENTS_TOTAL: IntCounterVec = IntCounterVec::new(
        prometheus::Opts::new("gita_vm_events_total", "Total events emitted"),
        &["kind"],
    ).unwrap();

    // ===== 调度指标 =====
    pub static ref METRIC_SCHEDULE_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("gita_vm_schedule_duration_seconds", "Scheduling decision latency"),
        &["result"],
    ).unwrap();

    // ===== 配额指标 =====
    pub static ref METRIC_QUOTA_USAGE: IntGaugeVec = IntGaugeVec::new(
        prometheus::Opts::new("gita_vm_quota_usage", "Quota usage"),
        &["scope", "scope_id", "resource"],
    ).unwrap();
}

/// 事件计数便捷函数（state.emit 使用）
pub fn inc_events_total(kind: &str) {
    METRIC_EVENTS_TOTAL.with_label_values(&[kind]).inc();
}

/// 指标注册表
#[derive(Clone)]
pub struct MetricsRegistry {
    registry: Arc<Registry>,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        let registry = Arc::new(Registry::new());

        let _ = registry.register(Box::new(METRIC_VMS_TOTAL.clone()));
        let _ = registry.register(Box::new(METRIC_VM_CREATED_TOTAL.clone()));
        let _ = registry.register(Box::new(METRIC_VM_DELETED_TOTAL.clone()));
        let _ = registry.register(Box::new(METRIC_NODES_TOTAL.clone()));
        let _ = registry.register(Box::new(METRIC_NODE_CPU_USAGE.clone()));
        let _ = registry.register(Box::new(METRIC_NODE_MEMORY_USAGE.clone()));
        let _ = registry.register(Box::new(METRIC_OPERATIONS_TOTAL.clone()));
        let _ = registry.register(Box::new(METRIC_OPERATION_DURATION.clone()));
        let _ = registry.register(Box::new(METRIC_IP_LEASES_TOTAL.clone()));
        let _ = registry.register(Box::new(METRIC_EVENTS_TOTAL.clone()));
        let _ = registry.register(Box::new(METRIC_SCHEDULE_DURATION.clone()));
        let _ = registry.register(Box::new(METRIC_QUOTA_USAGE.clone()));

        Self { registry }
    }

    /// 获取 /metrics 路由
    pub fn router(&self) -> Router {
        let registry = self.registry.clone();
        Router::new().route(
            "/metrics",
            axum::routing::get(move || {
                let registry = registry.clone();
                async move {
                    let encoder = TextEncoder::new();
                    let metric_families = registry.gather();
                    let mut buffer = Vec::new();
                    let _ = encoder.encode(&metric_families, &mut buffer);
                    axum::response::Response::builder()
                        .header("Content-Type", encoder.format_type())
                        .body(axum::body::Body::from(buffer))
                        .unwrap()
                }
            }),
        )
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_creates() {
        let _reg = MetricsRegistry::new();
    }
}
#[cfg(test)]
mod metrics_tests {
    use super::*;
    use tower::ServiceExt;

    /// Counting an event makes its counter grow, by exactly the number
    /// of events counted under that kind.
    #[test]
    fn counting_events_grows_the_counter() {
        let before = METRIC_EVENTS_TOTAL
            .with_label_values(&["test"])
            .get();
        inc_events_total("test");
        inc_events_total("test");
        inc_events_total("test");
        let after = METRIC_EVENTS_TOTAL.with_label_values(&["test"]).get();
        assert_eq!(after - before, 3, "each counted event must be counted once");
    }

    /// The metrics endpoint serves the metrics in the Prometheus text
    /// format.
    #[tokio::test]
    async fn the_metrics_endpoint_serves_prometheus_text() {
        // A vector metric exports nothing until one of its label
        // combinations is touched.
        METRIC_VMS_TOTAL.with_label_values(&["running"]).set(1);
        inc_events_total("test");

        let registry = MetricsRegistry::new();
        let app = registry.router();

        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/metrics")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(
            content_type.starts_with("text/plain"),
            "the exposition format is text, got {content_type}"
        );

        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("gita_vm_vms_total"),
            "the vm counter must be exported, got:\n{body}"
        );
        assert!(
            body.contains("gita_vm_events_total"),
            "the events counter must be exported, got:\n{body}"
        );
    }
}
