//! 控制面共享状态

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::auth::VmAuth;
use gita_vm_api::Event;
use gita_vm_core::Repository;
use gita_vm_obs::MetricsRegistry;
use tokio::sync::broadcast;
use tokio::sync::Mutex;

/// 控制面运行参数
#[derive(Debug, Clone)]
pub struct ControlConfig {
    /// reconcile 环间隔（默认 5 秒）
    pub reconcile_interval: Duration,
    /// rebalance 环间隔（默认 60 秒）
    pub rebalance_interval: Duration,
    /// 节点负载超过该比例视为过载
    pub overload_ratio: f64,
    /// 节点负载低于该比例视为可接收
    pub underload_ratio: f64,
    /// 是否自动执行再平衡建议（默认只记录事件）
    pub auto_rebalance: bool,
    /// 调用节点代理的超时时间
    pub agent_timeout: Duration,
    /// 版本号
    pub version: String,
    /// 节点代理通信模式
    pub agent_mode: AgentMode,
}

impl Default for ControlConfig {
    fn default() -> Self {
        ControlConfig {
            reconcile_interval: Duration::from_secs(5),
            rebalance_interval: Duration::from_secs(60),
            overload_ratio: 0.8,
            underload_ratio: 0.3,
            auto_rebalance: false,
            agent_timeout: Duration::from_secs(30),
            version: env!("CARGO_PKG_VERSION").to_string(),
            agent_mode: AgentMode::Polling,
        }
    }
}

/// 节点代理通信模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    /// 推模式：控制面主动 POST 到节点的 /agent/v1/execute（需节点开放入站端口）
    Push,
    /// 长轮询模式：节点长轮询 GET /internal/vm/operations/next（节点零入站端口）
    Polling,
}

/// 控制面共享状态
#[derive(Clone)]
pub struct AppState {
    /// 持久化后端
    pub repo: Arc<dyn Repository>,
    /// 调用节点代理的 HTTP 客户端
    pub http: reqwest::Client,
    /// 事件广播（供 SSE 推送）
    pub events: broadcast::Sender<Event>,
    /// 指标注册表
    pub metrics: Arc<MetricsRegistry>,
    /// 运行参数
    pub config: Arc<ControlConfig>,
    /// 事件序号分配器
    seq: Arc<AtomicU64>,
    /// 进程启动时间（毫秒）
    pub started_at_ms: i64,
    /// 长轮询状态：节点 ID → 待下发操作队列 + 唤醒器
    pub poll_state: Arc<Mutex<std::collections::HashMap<String, gita_vm_api::operation::Operation>>>,
    /// 鉴权上下文。
    ///
    /// 必填：控制面永远带鉴权运行，不存在"未接入鉴权"这条路径——
    /// 否则一个漏配的部署就会把 VM 管理接口裸奔在公网上。
    pub auth: VmAuth,
}

impl AppState {
    /// 用给定的存储后端、参数与鉴权上下文构造状态
    pub fn new(repo: Arc<dyn Repository>, config: ControlConfig, auth: VmAuth) -> Self {
        let (tx, _rx) = broadcast::channel(1024);
        let http = reqwest::Client::builder()
            .timeout(config.agent_timeout)
            .build()
            .unwrap_or_default();
        AppState {
            repo,
            http,
            events: tx,
            metrics: Arc::new(MetricsRegistry::new()),
            config: Arc::new(config),
            seq: Arc::new(AtomicU64::new(0)),
            started_at_ms: now_ms(),
            poll_state: Arc::new(Mutex::new(std::collections::HashMap::new())),
            auth,
        }
    }

    /// 分配下一个事件序号
    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// 记录并广播一个事件
    pub async fn emit(
        &self,
        kind: &str,
        object: Option<String>,
        message: impl Into<String>,
        payload: serde_json::Value,
    ) -> Event {
        let event = Event::new(self.next_seq(), kind, message, now_ms()).with_payload(payload);
        let event = match object {
            Some(o) => event.with_object(o),
            None => event,
        };
        if let Err(e) = self.repo.append_event(&event).await {
            tracing::warn!(kind, error = %e, "failed to write event");
        }
        gita_vm_obs::inc_events_total(kind);
        let _ = self.events.send(event.clone());
        event
    }
}

/// 当前 Unix 毫秒时间戳
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
#[cfg(test)]
mod compile_checks {
    use super::*;

    fn assert_send_sync<T: Send + Sync + Clone + 'static>() {}

    #[test]
    fn app_state_is_send_sync_clone() {
        assert_send_sync::<AppState>();
    }
}
