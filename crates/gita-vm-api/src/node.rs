//! 节点相关类型

use crate::ids::{CellId, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// VMM 后端类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum VmmKind {
    Qemu,
    CloudHypervisor,
    Firecracker,
}

impl VmmKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            VmmKind::Qemu => "qemu",
            VmmKind::CloudHypervisor => "cloud-hypervisor",
            VmmKind::Firecracker => "firecracker",
        }
    }
}

impl Default for VmmKind {
    fn default() -> Self {
        VmmKind::Qemu
    }
}

impl std::str::FromStr for VmmKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "qemu" => Ok(VmmKind::Qemu),
            "cloud-hypervisor" | "cloudhypervisor" | "ch" => Ok(VmmKind::CloudHypervisor),
            "firecracker" | "fc" => Ok(VmmKind::Firecracker),
            _ => Err(format!("unknown VMM kind: {s}")),
        }
    }
}

/// 节点能力
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NodeCapabilities {
    /// 支持的 VMM 列表
    pub vmm: Vec<VmmKind>,
    /// 支持的架构
    pub arch: Vec<String>,
    /// 是否支持嵌套虚拟化
    pub nested_virt: bool,
    /// 是否支持 GPU 直通
    pub gpu_passthrough: bool,
    /// 最大 VCPU 数
    pub max_vcpus: u32,
    /// 最大内存（字节）
    pub max_memory_bytes: u64,
}

/// 节点规格（静态容量）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NodeSpec {
    /// 总 CPU 毫核数
    pub cpu_millis: u64,
    /// 总内存字节数
    pub memory_bytes: u64,
    /// 总磁盘字节数
    pub disk_bytes: u64,
    /// 节点能力
    pub capabilities: NodeCapabilities,
}

/// 节点状态（动态）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NodeStatus {
    /// 已分配 CPU 毫核数
    pub alloc_cpu_millis: u64,
    /// 已分配内存字节数
    pub alloc_memory_bytes: u64,
    /// 已分配磁盘字节数
    pub alloc_disk_bytes: u64,
    /// 运行中 VM 数
    pub running_vms: u32,
    /// 当前阶段
    pub phase: NodePhase,
    /// 最后心跳时间（毫秒）
    pub last_heartbeat_ms: i64,
}

/// 节点阶段
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodePhase {
    /// 正在加入集群
    #[default]
    Joining,
    /// 正常在线
    Online,
    /// 维护模式（不调度新 VM）
    Maintenance,
    /// 腾空中（驱逐 VM 准备下线）
    Draining,
    /// 离线
    Offline,
}

/// 污点效果
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaintEffect {
    /// 拒绝调度
    NoSchedule,
    /// 尽量不调度
    PreferNoSchedule,
    /// 驱逐已有 VM
    NoExecute,
}

/// 污点
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Taint {
    pub key: String,
    pub value: String,
    pub effect: TaintEffect,
}

/// 标签选择器
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LabelSelector {
    /// 匹配的键
    pub key: String,
    /// 操作符
    pub operator: LabelOp,
    /// 匹配的值（In/NotIn/Exists 时使用）
    #[serde(default)]
    pub values: Vec<String>,
}

/// 标签操作符
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LabelOp {
    In,
    NotIn,
    Exists,
    DoesNotExist,
}

/// 节点条件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeConditionType {
    Ready,
    NetworkReady,
    DiskPressure,
    MemoryPressure,
    PidPressure,
}

/// 条件状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConditionStatus {
    True,
    False,
    Unknown,
}

/// 节点条件
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NodeCondition {
    pub kind: NodeConditionType,
    pub status: ConditionStatus,
    pub last_transition_ms: i64,
    pub reason: Option<String>,
    pub message: Option<String>,
}

/// 节点 Agent 端点信息
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AgentEndpoint {
    pub url: String,
    pub version: String,
    pub capabilities: NodeCapabilities,
}

/// 一个节点
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    /// 所属 Cell
    #[serde(default)]
    pub cell: Option<CellId>,
    /// 规格
    pub spec: NodeSpec,
    /// 状态
    pub status: NodeStatus,
    /// 污点
    #[serde(default)]
    pub taints: Vec<Taint>,
    /// 标签
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// 资源归属
    pub owner: crate::ids::OwnerRef,
    /// 条件
    #[serde(default)]
    pub conditions: Vec<NodeCondition>,
    /// 创建时间（毫秒）
    pub created_at_ms: i64,
    /// 最后更新时间（毫秒）
    pub updated_at_ms: i64,
}

impl Node {
    /// 可分配资源 = 总量 - 已分配 - 系统预留
    pub fn allocatable(&self, system_reserved: &Resources) -> Resources {
        Resources {
            cpu_millis: self.spec.cpu_millis.saturating_sub(self.status.alloc_cpu_millis).saturating_sub(system_reserved.cpu_millis),
            memory_bytes: self.spec.memory_bytes.saturating_sub(self.status.alloc_memory_bytes).saturating_sub(system_reserved.memory_bytes),
            disk_bytes: self.spec.disk_bytes.saturating_sub(self.status.alloc_disk_bytes).saturating_sub(system_reserved.disk_bytes),
        }
    }

    /// 是否可调度
    pub fn is_schedulable(&self) -> bool {
        matches!(self.status.phase, NodePhase::Online) && self.is_healthy()
    }

    /// 是否健康
    pub fn is_healthy(&self) -> bool {
        self.conditions.iter().any(|c| c.kind == NodeConditionType::Ready && c.status == ConditionStatus::True)
    }

    /// 负载比例（取 CPU/内存/磁盘最大值）
    pub fn load_ratio(&self) -> f64 {
        let cpu = if self.spec.cpu_millis > 0 {
            self.status.alloc_cpu_millis as f64 / self.spec.cpu_millis as f64
        } else { 0.0 };
        let mem = if self.spec.memory_bytes > 0 {
            self.status.alloc_memory_bytes as f64 / self.spec.memory_bytes as f64
        } else { 0.0 };
        let disk = if self.spec.disk_bytes > 0 {
            self.status.alloc_disk_bytes as f64 / self.spec.disk_bytes as f64
        } else { 0.0 };
        cpu.max(mem).max(disk)
    }
}

/// 资源量
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Resources {
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
}

impl Resources {
    pub fn new(cpu_millis: u64, memory_bytes: u64, disk_bytes: u64) -> Self {
        Self { cpu_millis, memory_bytes, disk_bytes }
    }

    pub fn zero() -> Self {
        Self::default()
    }

    pub fn add(&self, other: &Resources) -> Resources {
        Resources {
            cpu_millis: self.cpu_millis.saturating_add(other.cpu_millis),
            memory_bytes: self.memory_bytes.saturating_add(other.memory_bytes),
            disk_bytes: self.disk_bytes.saturating_add(other.disk_bytes),
        }
    }

    pub fn sub(&self, other: &Resources) -> Resources {
        Resources {
            cpu_millis: self.cpu_millis.saturating_sub(other.cpu_millis),
            memory_bytes: self.memory_bytes.saturating_sub(other.memory_bytes),
            disk_bytes: self.disk_bytes.saturating_sub(other.disk_bytes),
        }
    }

    pub fn fits(&self, required: &Resources) -> bool {
        self.cpu_millis >= required.cpu_millis
            && self.memory_bytes >= required.memory_bytes
            && self.disk_bytes >= required.disk_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_arithmetic() {
        let a = Resources::new(1000, 1_000_000, 10_000_000);
        let b = Resources::new(500, 500_000, 5_000_000);
        assert_eq!(a.add(&b).cpu_millis, 1500);
        assert_eq!(a.sub(&b).memory_bytes, 500_000);
    }

    #[test]
    fn resources_fits() {
        let avail = Resources::new(4000, 4_000_000_000, 20_000_000_000);
        let need = Resources::new(2000, 2_000_000_000, 10_000_000_000);
        assert!(avail.fits(&need));
        let need_big = Resources::new(5000, 2_000_000_000, 10_000_000_000);
        assert!(!avail.fits(&need_big));
    }

    #[test]
    fn node_load_ratio() {
        let node = Node {
            id: NodeId(1),
            name: "test".into(),
            cell: None,
            spec: NodeSpec {
                cpu_millis: 10000,
                memory_bytes: 16_000_000_000,
                disk_bytes: 500_000_000_000,
                capabilities: NodeCapabilities::default(),
            },
            status: NodeStatus {
                alloc_cpu_millis: 5000,
                alloc_memory_bytes: 8_000_000_000,
                alloc_disk_bytes: 100_000_000_000,
                running_vms: 2,
                phase: NodePhase::Online,
                last_heartbeat_ms: 0,
            },
            taints: vec![],
            labels: BTreeMap::new(),
            owner: crate::ids::OwnerRef::system(),
            conditions: vec![NodeCondition {
                kind: NodeConditionType::Ready,
                status: ConditionStatus::True,
                last_transition_ms: 0,
                reason: None,
                message: None,
            }],
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        // CPU 50%, Memory 50%, Disk 20% -> max = 0.5
        assert!((node.load_ratio() - 0.5).abs() < 0.01);
        assert!(node.is_schedulable());
    }
}