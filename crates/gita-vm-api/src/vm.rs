//! 虚拟机核心类型：VmSpec, Vm, VmStatus, VmCondition

use crate::ids::{NodeId, TemplateId, VmId};
use crate::network::NetworkAttachment;
use crate::node::{LabelSelector, TaintEffect, VmmKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 镜像引用：名字 + 可选摘要
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ImageRef {
    /// 镜像名，如 `ubuntu-24.04`
    pub name: String,
    /// 内容摘要，用于确保拉取的是同一份镜像
    #[serde(default)]
    pub digest: Option<String>,
}

/// 虚拟机的期望规格
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VmSpec {
    /// CPU 毫核数
    pub cpu_millis: u64,
    /// 内存字节数
    pub memory_bytes: u64,
    /// 根盘字节数
    pub disk_bytes: u64,
    /// 使用哪个镜像（rootfs）
    pub image: ImageRef,
    /// 内核镜像引用；`None` = 使用平台默认内核
    #[serde(default)]
    pub kernel: Option<ImageRef>,
    /// 网络挂载
    #[serde(default)]
    pub networks: Vec<NetworkAttachment>,
    /// VMM 后端
    pub backend: VmmKind,
    /// 要求的宿主架构（如 `x86_64` / `aarch64`）；为空表示不限制
    #[serde(default)]
    pub arch: Option<String>,
    /// 用户数据（注入 guest 的 metadata 服务 / cloud-init）
    #[serde(default)]
    pub user_data: Option<String>,
    /// 注入 guest 的额外 metadata 键值
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// 由哪个模板实例化而来
    #[serde(default)]
    pub template: Option<TemplateId>,
    /// 亲和性规则
    #[serde(default)]
    pub affinity: Option<AffinitySpec>,
    /// 污点容忍
    #[serde(default)]
    pub tolerations: Vec<Toleration>,
    /// 优先级，数值越大越优先被调度/迁移
    #[serde(default)]
    pub priority: i32,
}

impl VmSpec {
    /// 规格占用的资源量
    pub fn resources(&self) -> crate::node::Resources {
        crate::node::Resources::new(self.cpu_millis, self.memory_bytes, self.disk_bytes)
    }
}

/// 亲和性规则
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AffinitySpec {
    /// 必须满足的规则（全部满足才可调度）
    #[serde(default)]
    pub required: Vec<LabelSelector>,
    /// 尽量满足的规则（加分项）
    #[serde(default)]
    pub preferred: Vec<LabelSelector>,
    /// 反亲和（尽量避开已运行满足这些选择器的虚机的节点）
    #[serde(default)]
    pub anti: Vec<LabelSelector>,
}

/// 容忍：允许虚拟机调度到带特定污点的节点
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Toleration {
    /// 容忍的污点键
    pub key: String,
    /// 容忍的效果；为空表示容忍所有效果
    #[serde(default)]
    pub effect: Option<TaintEffect>,
    /// 容忍时长（秒）；`None` 表示永久
    #[serde(default)]
    pub toleration_seconds: Option<i64>,
}

/// 虚拟机状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VmStatus {
    /// 已创建，等待调度
    Pending,
    /// 已调度，正在创建资源
    Provisioning,
    /// 运行中
    Running,
    /// 正在停止
    Stopping,
    /// 已停止
    Stopped,
    /// 正在迁移
    Migrating,
    /// 创建或运行失败
    Failed,
    /// 反复失败被熔断，等待人工介入
    Quarantined,
    /// 已删除（资源已回收）
    Deleted,
}

impl VmStatus {
    /// 是否是终态（不会再自动变化）
    pub fn is_terminal(&self) -> bool {
        matches!(self, VmStatus::Failed | VmStatus::Quarantined | VmStatus::Deleted)
    }

    /// 是否处于活跃状态（占用节点资源）
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            VmStatus::Provisioning | VmStatus::Running | VmStatus::Stopping | VmStatus::Migrating
        )
    }
}

/// 虚拟机条件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VmConditionType {
    /// 资源是否就绪
    Ready,
    /// 网络是否已配置
    NetworkReady,
    /// 元数据是否已注入
    MetadataApplied,
}

/// 单个虚拟机条件
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VmCondition {
    pub kind: VmConditionType,
    pub status: crate::node::ConditionStatus,
    pub last_transition_ms: i64,
    pub reason: Option<String>,
}

/// 一个虚拟机实例
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Vm {
    pub id: VmId,
    pub name: String,
    /// 资源归属
    pub owner: crate::ids::OwnerRef,
    /// 所属 Cell（预留，未来多区域用）
    #[serde(default)]
    pub cell: Option<crate::ids::CellId>,
    /// 落位的节点；未调度时为空
    #[serde(default)]
    pub node: Option<NodeId>,
    pub spec: VmSpec,
    pub status: VmStatus,
    /// 期望状态版本号，每次 spec 变更 +1
    pub generation: u64,
    /// 已经实际达成的版本号。与 generation 不一致表示变更尚未生效。
    pub observed_generation: u64,
    #[serde(default)]
    pub conditions: Vec<VmCondition>,
    /// 主机侧存放 rootfs 的路径
    #[serde(default)]
    pub rootfs_path: Option<String>,
    /// 分配的 IP 地址
    #[serde(default)]
    pub ip_address: Option<String>,
    /// VNC/控制台访问地址
    #[serde(default)]
    pub console_url: Option<String>,
    /// 创建时间（毫秒）
    pub created_at_ms: i64,
    /// 最近状态说明
    #[serde(default)]
    pub message: Option<String>,
    /// 过期时间（毫秒），用于临时 CI VM 自动回收
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
}

impl Vm {
    /// 期望是否已被实际状态追上
    pub fn is_converged(&self) -> bool {
        self.generation == self.observed_generation
    }

    /// 是否已过期
    pub fn is_expired(&self, now_ms: i64) -> bool {
        self.expires_at_ms.map(|e| e < now_ms).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_spec_resources() {
        let spec = VmSpec {
            cpu_millis: 2000,
            memory_bytes: 2_000_000_000,
            disk_bytes: 10_000_000_000,
            ..Default::default()
        };
        let res = spec.resources();
        assert_eq!(res.cpu_millis, 2000);
        assert_eq!(res.memory_bytes, 2_000_000_000);
        assert_eq!(res.disk_bytes, 10_000_000_000);
    }

    #[test]
    fn vm_status_properties() {
        assert!(VmStatus::Running.is_active());
        assert!(!VmStatus::Running.is_terminal());
        assert!(VmStatus::Failed.is_terminal());
        assert!(!VmStatus::Failed.is_active());
        assert!(VmStatus::Pending.is_terminal() == false);
        assert!(VmStatus::Pending.is_active() == false);
    }

    #[test]
    fn vm_converged() {
        let mut vm = Vm {
            id: VmId(1),
            name: "test".into(),
            owner: crate::ids::OwnerRef::system(),
            cell: None,
            node: None,
            spec: VmSpec::default(),
            status: VmStatus::Running,
            generation: 3,
            observed_generation: 3,
            conditions: vec![],
            rootfs_path: None,
            ip_address: None,
            console_url: None,
            created_at_ms: 0,
            message: None,
            expires_at_ms: None,
        };
        assert!(vm.is_converged());
        vm.generation = 4;
        assert!(!vm.is_converged());
    }
}