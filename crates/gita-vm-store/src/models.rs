//! 数据库行模型（sqlx FromRow）

use gita_vm_api::ids::{ImageId, NetworkId, NodeId, OwnerRef, TemplateId, VmId};
use gita_vm_api::network::{IpLease, LeaseState, NetProvider, Network};
use gita_vm_api::node::{Node, NodeCapabilities, NodePhase, NodeSpec, NodeStatus, VmmKind};
use gita_vm_api::operation::{Operation, OperationKind, OperationStatus};
use gita_vm_api::quota::{Quota, QuotaScope};
use gita_vm_api::template::VmTemplate;
use gita_vm_api::vm::{Vm, VmStatus};
use gita_vm_api::image::{Image, ImageKind, ImageStatus};
use serde_json::Value;
use sqlx::FromRow;
use std::str::FromStr;

/// OwnerRef 在 DB 中存为两列
#[derive(Debug, Clone, FromRow)]
pub struct DbOwnerRef {
    pub owner_type: String,
    pub owner_id: i64,
}

impl From<DbOwnerRef> for OwnerRef {
    fn from(o: DbOwnerRef) -> Self {
        Self {
            owner_type: match o.owner_type.as_str() {
                "group" => gita_vm_api::ids::OwnerType::Group,
                "repo" => gita_vm_api::ids::OwnerType::Repo,
                _ => gita_vm_api::ids::OwnerType::System,
            },
            owner_id: o.owner_id,
        }
    }
}

impl From<OwnerRef> for DbOwnerRef {
    fn from(o: OwnerRef) -> Self {
        Self {
            owner_type: match o.owner_type {
                gita_vm_api::ids::OwnerType::System => "system".into(),
                gita_vm_api::ids::OwnerType::Group => "group".into(),
                gita_vm_api::ids::OwnerType::Repo => "repo".into(),
            },
            owner_id: o.owner_id,
        }
    }
}

/// 网络行
#[derive(Debug, FromRow)]
pub struct DbNetwork {
    pub id: i64,
    pub name: String,
    pub provider: String,
    pub bridge: String,
    pub cidr: String,
    pub gateway: String,
    pub dhcp_start: Option<String>,
    pub dhcp_end: Option<String>,
    pub mtu: i32,
    pub vlan: Option<i16>,
    pub owner_type: String,
    pub owner_id: i64,
    pub is_default: bool,
    pub created_at_ms: i64,
}

impl From<DbNetwork> for Network {
    fn from(n: DbNetwork) -> Self {
        Self {
            id: NetworkId(n.id),
            name: n.name,
            provider: match n.provider.as_str() {
                "macvtap" => NetProvider::MacVTap,
                "passt" => NetProvider::Passt,
                "cni" => NetProvider::Cni,
                _ => NetProvider::Bridge,
            },
            bridge: n.bridge,
            cidr: n.cidr,
            gateway: n.gateway,
            dhcp_start: n.dhcp_start,
            dhcp_end: n.dhcp_end,
            mtu: n.mtu as u32,
            vlan: n.vlan.map(|v| v as u16),
            owner: DbOwnerRef { owner_type: n.owner_type, owner_id: n.owner_id }.into(),
            is_default: n.is_default,
            created_at_ms: n.created_at_ms,
        }
    }
}

/// IP 租约行
#[derive(Debug, FromRow)]
pub struct DbIpLease {
    pub id: i64,
    pub network_id: i64,
    pub ip: String,
    pub mac: String,
    pub vm_id: Option<i64>,
    pub node_id: Option<i64>,
    pub state: String,
    pub expires_at_ms: Option<i64>,
    pub created_at_ms: i64,
}

impl From<DbIpLease> for IpLease {
    fn from(l: DbIpLease) -> Self {
        Self {
            id: l.id,
            network_id: NetworkId(l.network_id),
            ip: l.ip,
            mac: l.mac,
            vm_id: l.vm_id.map(VmId),
            node_id: l.node_id.map(NodeId),
            state: match l.state.as_str() {
                "reserved" => LeaseState::Reserved,
                "released" => LeaseState::Released,
                "expired" => LeaseState::Expired,
                _ => LeaseState::Allocated,
            },
            expires_at_ms: l.expires_at_ms,
            created_at_ms: l.created_at_ms,
        }
    }
}

/// 节点行
#[derive(Debug, FromRow)]
pub struct DbNode {
    pub id: i64,
    pub name: String,
    pub cell_id: Option<i64>,
    pub cpu_millis: i64,
    pub memory_bytes: i64,
    pub disk_bytes: i64,
    pub capabilities_vmm: Vec<String>,
    pub capabilities_arch: Vec<String>,
    pub nested_virt: bool,
    pub gpu_passthrough: bool,
    pub max_vcpus: i32,
    pub max_memory_bytes: i64,
    pub alloc_cpu_millis: i64,
    pub alloc_memory_bytes: i64,
    pub alloc_disk_bytes: i64,
    pub running_vms: i32,
    pub phase: String,
    pub last_heartbeat_ms: i64,
    pub taints: Value,
    pub labels: Value,
    pub owner_type: String,
    pub owner_id: i64,
    pub conditions: Value,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl From<DbNode> for Node {
    fn from(n: DbNode) -> Self {
        Self {
            id: NodeId(n.id),
            name: n.name,
            cell: n.cell_id.map(gita_vm_api::ids::CellId),
            spec: NodeSpec {
                cpu_millis: n.cpu_millis as u64,
                memory_bytes: n.memory_bytes as u64,
                disk_bytes: n.disk_bytes as u64,
                capabilities: NodeCapabilities {
                    vmm: n.capabilities_vmm.into_iter().filter_map(|s| VmmKind::from_str(&s).ok()).collect(),
                    arch: n.capabilities_arch,
                    nested_virt: n.nested_virt,
                    gpu_passthrough: n.gpu_passthrough,
                    max_vcpus: n.max_vcpus as u32,
                    max_memory_bytes: n.max_memory_bytes as u64,
                },
            },
            status: NodeStatus {
                alloc_cpu_millis: n.alloc_cpu_millis as u64,
                alloc_memory_bytes: n.alloc_memory_bytes as u64,
                alloc_disk_bytes: n.alloc_disk_bytes as u64,
                running_vms: n.running_vms as u32,
                phase: match n.phase.as_str() {
                    "online" => NodePhase::Online,
                    "maintenance" => NodePhase::Maintenance,
                    "draining" => NodePhase::Draining,
                    "offline" => NodePhase::Offline,
                    _ => NodePhase::Joining,
                },
                last_heartbeat_ms: n.last_heartbeat_ms,
            },
            taints: serde_json::from_value(n.taints).unwrap_or_default(),
            labels: serde_json::from_value(n.labels).unwrap_or_default(),
            owner: DbOwnerRef { owner_type: n.owner_type, owner_id: n.owner_id }.into(),
            conditions: serde_json::from_value(n.conditions).unwrap_or_default(),
            created_at_ms: n.created_at_ms,
            updated_at_ms: n.updated_at_ms,
        }
    }
}

/// 镜像行
#[derive(Debug, FromRow)]
pub struct DbImage {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub status: String,
    pub pool_id: Option<i64>,
    pub path: String,
    pub size_bytes: i64,
    pub digest: String,
    pub format: String,
    pub arch: String,
    pub owner_type: String,
    pub owner_id: i64,
    pub tags: Vec<String>,
    pub metadata: Value,
    pub created_at_ms: i64,
    pub last_used_at_ms: Option<i64>,
}

impl From<DbImage> for Image {
    fn from(i: DbImage) -> Self {
        Self {
            id: ImageId(i.id),
            name: i.name,
            kind: match i.kind.as_str() {
                "kernel" => ImageKind::Kernel,
                "cloud-init" => ImageKind::CloudInit,
                _ => ImageKind::Rootfs,
            },
            status: match i.status.as_str() {
                "ready" => ImageStatus::Ready,
                "failed" => ImageStatus::Failed,
                "deleting" => ImageStatus::Deleting,
                _ => ImageStatus::Importing,
            },
            pool_id: i.pool_id.map(gita_vm_api::ids::StoragePoolId).unwrap_or(gita_vm_api::ids::StoragePoolId(0)),
            path: i.path,
            size_bytes: i.size_bytes as u64,
            digest: i.digest,
            format: i.format,
            arch: i.arch,
            owner: DbOwnerRef { owner_type: i.owner_type, owner_id: i.owner_id }.into(),
            tags: i.tags,
            metadata: serde_json::from_value(i.metadata).unwrap_or_default(),
            created_at_ms: i.created_at_ms,
            last_used_at_ms: i.last_used_at_ms,
        }
    }
}

/// 模板行
#[derive(Debug, FromRow)]
pub struct DbTemplate {
    pub id: i64,
    pub name: String,
    pub version: i64,
    pub spec: Value,
    pub image_id: Option<i64>,
    pub kernel_image_id: Option<i64>,
    pub owner_type: String,
    pub owner_id: i64,
    pub tags: Vec<String>,
    pub description: String,
    pub created_at_ms: i64,
}

impl From<DbTemplate> for VmTemplate {
    fn from(t: DbTemplate) -> Self {
        Self {
            id: TemplateId(t.id),
            name: t.name,
            version: t.version as u64,
            spec: serde_json::from_value(t.spec).unwrap_or_default(),
            image_id: t.image_id.map(ImageId),
            kernel_image_id: t.kernel_image_id.map(ImageId),
            owner: DbOwnerRef { owner_type: t.owner_type, owner_id: t.owner_id }.into(),
            tags: t.tags,
            description: t.description,
            created_at_ms: t.created_at_ms,
        }
    }
}

/// VM 实例行
#[derive(Debug, FromRow)]
pub struct DbVm {
    pub id: i64,
    pub name: String,
    pub template_id: Option<i64>,
    pub owner_type: String,
    pub owner_id: i64,
    pub repo_id: Option<i64>,
    pub pipeline_run_id: Option<i64>,
    pub job_id: Option<i64>,
    pub node_id: Option<i64>,
    pub spec: Value,
    pub status: String,
    pub generation: i64,
    pub observed_generation: i64,
    pub conditions: Value,
    pub rootfs_path: Option<String>,
    pub ip_address: Option<String>,
    pub console_url: Option<String>,
    pub created_at_ms: i64,
    pub message: Option<String>,
    pub expires_at_ms: Option<i64>,
}

impl From<DbVm> for Vm {
    fn from(v: DbVm) -> Self {
        Self {
            id: VmId(v.id),
            name: v.name,
            owner: DbOwnerRef { owner_type: v.owner_type, owner_id: v.owner_id }.into(),
            cell: None,
            node: v.node_id.map(NodeId),
            spec: serde_json::from_value(v.spec).unwrap_or_default(),
            status: match v.status.as_str() {
                "provisioning" => VmStatus::Provisioning,
                "running" => VmStatus::Running,
                "stopping" => VmStatus::Stopping,
                "stopped" => VmStatus::Stopped,
                "migrating" => VmStatus::Migrating,
                "failed" => VmStatus::Failed,
                "quarantined" => VmStatus::Quarantined,
                _ => VmStatus::Pending,
            },
            generation: v.generation as u64,
            observed_generation: v.observed_generation as u64,
            conditions: serde_json::from_value(v.conditions).unwrap_or_default(),
            rootfs_path: v.rootfs_path,
            ip_address: v.ip_address,
            console_url: v.console_url,
            created_at_ms: v.created_at_ms,
            message: v.message,
            expires_at_ms: v.expires_at_ms,
        }
    }
}

/// 操作行
#[derive(Debug, FromRow)]
pub struct DbOperation {
    pub id: i64,
    pub kind: String,
    pub vm_id: i64,
    pub node_id: Option<i64>,
    pub steps: Value,
    pub current_step: i32,
    pub status: String,
    pub priority: i32,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
    pub error: Option<String>,
}

impl From<DbOperation> for Operation {
    fn from(o: DbOperation) -> Self {
        Self {
            id: gita_vm_api::ids::OperationId(o.id),
            kind: match o.kind.as_str() {
                "start_vm" => OperationKind::StartVm,
                "stop_vm" => OperationKind::StopVm,
                "restart_vm" => OperationKind::RestartVm,
                "delete_vm" => OperationKind::DeleteVm,
                "migrate_vm" => OperationKind::MigrateVm,
                "create_snapshot" => OperationKind::CreateSnapshot,
                "restore_snapshot" => OperationKind::RestoreSnapshot,
                "resize_disk" => OperationKind::ResizeDisk,
                "hotplug_resources" => OperationKind::HotplugResources,
                "update_network" => OperationKind::UpdateNetwork,
                "inject_user_data" => OperationKind::InjectUserData,
                _ => OperationKind::CreateVm,
            },
            vm_id: VmId(o.vm_id),
            node_id: o.node_id.map(NodeId),
            steps: serde_json::from_value(o.steps).unwrap_or_default(),
            current_step: o.current_step as usize,
            status: match o.status.as_str() {
                "running" => OperationStatus::Running,
                "succeeded" => OperationStatus::Succeeded,
                "failed" => OperationStatus::Failed,
                "cancelling" => OperationStatus::Cancelling,
                "cancelled" => OperationStatus::Cancelled,
                _ => OperationStatus::Pending,
            },
            created_at_ms: o.created_at_ms,
            started_at_ms: o.started_at_ms,
            finished_at_ms: o.finished_at_ms,
            error: o.error,
            priority: o.priority,
        }
    }
}

/// 配额行
#[derive(Debug, FromRow)]
pub struct DbQuota {
    pub id: i64,
    pub scope: String,
    pub scope_id: i64,
    pub max_concurrent_vms: i32,
    pub max_cpu_millis: i64,
    pub max_memory_bytes: i64,
    pub max_disk_bytes: i64,
    pub max_vm_lifetime_ms: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl From<DbQuota> for Quota {
    fn from(q: DbQuota) -> Self {
        Self {
            id: q.id,
            scope: match q.scope.as_str() {
                "group" => QuotaScope::Group,
                "repo" => QuotaScope::Repo,
                "user" => QuotaScope::User,
                _ => QuotaScope::System,
            },
            scope_id: q.scope_id,
            max_concurrent_vms: q.max_concurrent_vms as u32,
            max_cpu_millis: q.max_cpu_millis as u64,
            max_memory_bytes: q.max_memory_bytes as u64,
            max_disk_bytes: q.max_disk_bytes as u64,
            max_vm_lifetime_ms: q.max_vm_lifetime_ms,
            created_at_ms: q.created_at_ms,
            updated_at_ms: q.updated_at_ms,
        }
    }
}