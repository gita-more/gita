//! # gita-vm-api
//!
//! gita VM 集群管理的公共类型契约。
//!
//! 设计原则：
//! 1. **spec / status 分离**：`spec` 是期望状态（用户写的），`status` 是实际状态（系统观测的）
//! 2. **generation / observed_generation**：期望版本号与已达成版本号，不等表示变更尚未落地
//! 3. **只用整数表示资源**：CPU 用毫核，内存/磁盘用字节，避免浮点比较
//! 4. **ID 类型统一用 i64**，兼容 gita 现有数据库主键
//! 5. **权限归属**：VM/模板/镜像都有 owner_type + owner_id，纳入 gita 权限体系

pub mod event;
pub mod ids;
pub mod image;
pub mod network;
pub mod node;
pub mod operation;
pub mod quota;
pub mod template;
pub mod vm;

pub use event::Event;
pub use ids::{ImageId, NetworkId, NodeId, OwnerRef, OwnerType, TemplateId, VmId, VolumeId};
pub use image::{Image, ImageKind, ImageStatus};
pub use network::{IpLease, LeaseState, NetProvider, Network, NetworkAttachment};
pub use node::{
    ConditionStatus, LabelOp, LabelSelector, Node, NodeCapabilities, NodeCondition,
    NodeConditionType, NodePhase, NodeSpec, NodeStatus, Resources, Taint, TaintEffect, VmmKind,
};
pub use operation::{
    now_ms, Operation, OperationKind, OperationStatus, OperationStep, StepKind, StepStatus,
};
pub use quota::{EffectiveQuota, Quota, QuotaExceeded, QuotaScope, QuotaUsage};
pub use template::{TemplateSpec, VmSpecOverrides, VmTemplate};
pub use vm::{
    AffinitySpec, ImageRef, Toleration, Vm, VmCondition, VmConditionType, VmSpec, VmStatus,
};