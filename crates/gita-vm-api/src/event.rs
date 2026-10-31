//! 事件类型：VM 领域事件（控制面内部广播用）

use serde::{Deserialize, Serialize};

/// 一个 VM 领域事件
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// 事件序号（由控制面分配）
    pub seq: u64,
    /// 事件类型（如 "vm.created"）
    pub kind: String,
    /// 事件描述
    pub message: String,
    /// 关联对象（如 VM ID）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    /// 毫秒时间戳
    pub timestamp_ms: i64,
    /// 结构化负载
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl Event {
    pub fn new(seq: u64, kind: impl Into<String>, message: impl Into<String>, timestamp_ms: i64) -> Self {
        Self {
            seq,
            kind: kind.into(),
            message: message.into(),
            object: None,
            timestamp_ms,
            payload: serde_json::Value::Null,
        }
    }

    pub fn with_object(mut self, object: impl Into<String>) -> Self {
        self.object = Some(object.into());
        self
    }

    pub fn with_payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = payload;
        self
    }
}