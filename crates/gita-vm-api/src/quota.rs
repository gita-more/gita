//! 配额管理类型

use crate::ids::OwnerRef;
use serde::{Deserialize, Serialize};

/// 配额范围
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuotaScope {
    /// 系统全局
    System,
    /// 群组
    Group,
    /// 仓库
    Repo,
    /// 用户
    User,
}

/// 一条配额
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Quota {
    pub id: i64,
    pub scope: QuotaScope,
    /// scope 对应的 ID（group_id / repo_id / user_id），system 时为 0
    pub scope_id: i64,
    /// 最大并发 VM 数
    pub max_concurrent_vms: u32,
    /// 总 CPU 毫核
    pub max_cpu_millis: u64,
    /// 总内存字节
    pub max_memory_bytes: u64,
    /// 总磁盘字节
    pub max_disk_bytes: u64,
    /// VM 最大存活时间（毫秒），0 = 不限制
    pub max_vm_lifetime_ms: i64,
    /// 创建时间（毫秒）
    pub created_at_ms: i64,
    /// 更新时间（毫秒）
    pub updated_at_ms: i64,
}

/// 配额使用情况
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QuotaUsage {
    pub concurrent_vms: u32,
    pub used_cpu_millis: u64,
    pub used_memory_bytes: u64,
    pub used_disk_bytes: u64,
}

impl Quota {
    /// 检查一个新建 VM 是否会超出配额
    ///
    /// 返回 Ok(()) 表示可以创建，Err(原因) 表示超限
    pub fn check(&self, usage: &QuotaUsage, requested: &crate::node::Resources) -> Result<(), QuotaExceeded> {
        if usage.concurrent_vms + 1 > self.max_concurrent_vms {
            return Err(QuotaExceeded::ConcurrentVms {
                limit: self.max_concurrent_vms,
                current: usage.concurrent_vms,
            });
        }
        if usage.used_cpu_millis + requested.cpu_millis > self.max_cpu_millis {
            return Err(QuotaExceeded::Cpu {
                limit: self.max_cpu_millis,
                requested: usage.used_cpu_millis + requested.cpu_millis,
            });
        }
        if usage.used_memory_bytes + requested.memory_bytes > self.max_memory_bytes {
            return Err(QuotaExceeded::Memory {
                limit: self.max_memory_bytes,
                requested: usage.used_memory_bytes + requested.memory_bytes,
            });
        }
        if usage.used_disk_bytes + requested.disk_bytes > self.max_disk_bytes {
            return Err(QuotaExceeded::Disk {
                limit: self.max_disk_bytes,
                requested: usage.used_disk_bytes + requested.disk_bytes,
            });
        }
        Ok(())
    }
}

/// 超配额错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuotaExceeded {
    #[error("并发 VM 数超限：当前 {current}，上限 {limit}")]
    ConcurrentVms { limit: u32, current: u32 },
    #[error("CPU 配额超限：申请后 {requested} 毫核，上限 {limit} 毫核")]
    Cpu { limit: u64, requested: u64 },
    #[error("内存配额超限：申请后 {requested} 字节，上限 {limit} 字节")]
    Memory { limit: u64, requested: u64 },
    #[error("磁盘配额超限：申请后 {requested} 字节，上限 {limit} 字节")]
    Disk { limit: u64, requested: u64 },
}

/// 适用于某个归属的配额（可能为系统默认）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveQuota {
    pub quota: Quota,
    pub usage: QuotaUsage,
    pub owner: OwnerRef,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::Resources;

    fn quota() -> Quota {
        Quota {
            id: 1,
            scope: QuotaScope::Group,
            scope_id: 42,
            max_concurrent_vms: 2,
            max_cpu_millis: 8000,
            max_memory_bytes: 8_000_000_000,
            max_disk_bytes: 50_000_000_000,
            max_vm_lifetime_ms: 4 * 3600 * 1000,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn allows_within_quota() {
        let q = quota();
        let usage = QuotaUsage { concurrent_vms: 1, ..Default::default() };
        let req = Resources::new(2000, 2_000_000_000, 10_000_000_000);
        assert!(q.check(&usage, &req).is_ok());
    }

    #[test]
    fn rejects_over_concurrent_vms() {
        let q = quota();
        let usage = QuotaUsage { concurrent_vms: 2, ..Default::default() };
        let req = Resources::new(2000, 2_000_000_000, 10_000_000_000);
        assert!(matches!(q.check(&usage, &req), Err(QuotaExceeded::ConcurrentVms { .. })));
    }

    #[test]
    fn rejects_over_cpu() {
        let q = quota();
        let usage = QuotaUsage { used_cpu_millis: 6000, ..Default::default() };
        let req = Resources::new(4000, 2_000_000_000, 10_000_000_000);
        assert!(matches!(q.check(&usage, &req), Err(QuotaExceeded::Cpu { .. })));
    }

    #[test]
    fn rejects_over_memory() {
        let q = quota();
        let usage = QuotaUsage { used_memory_bytes: 7_000_000_000, ..Default::default() };
        let req = Resources::new(2000, 2_000_000_000, 10_000_000_000);
        assert!(matches!(q.check(&usage, &req), Err(QuotaExceeded::Memory { .. })));
    }

    #[test]
    fn rejects_over_disk() {
        let q = quota();
        let usage = QuotaUsage { used_disk_bytes: 45_000_000_000, ..Default::default() };
        let req = Resources::new(2000, 2_000_000_000, 10_000_000_000);
        assert!(matches!(q.check(&usage, &req), Err(QuotaExceeded::Disk { .. })));
    }
}