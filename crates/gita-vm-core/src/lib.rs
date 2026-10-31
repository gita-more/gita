//! # gita-vm-core
//!
//! gita VM 集群的**领域核心**。这一层是纯逻辑，不依赖数据库、HTTP 或具体的 VMM，
//! 因此可以被完整地单元测试。
//!
//! 包含：
//!
//! - [`scheduler`]：调度器（filter → score → 选点）
//! - [`state`]：虚拟机状态机
//! - [`operation`]：操作引擎（有序、幂等的步骤）
//! - [`repository`]：持久化契约（trait）
//! - [`ipam`]：IP 地址管理
//! - [`quota`]：配额引擎（分层校验）

pub mod error;
pub mod ipam;
pub mod operation;
pub mod quota;
pub mod repository;
pub mod scheduler;
pub mod state;

pub use error::{Error, Result};
pub use repository::Repository;

/// 当前 Unix 毫秒时间戳
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}