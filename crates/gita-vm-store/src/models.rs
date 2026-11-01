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
#[cfg(test)]
mod tests {
    use super::*;
    use gita_vm_api::ids::OwnerType;
    use gita_vm_api::network::{LeaseState, NetProvider};
    use gita_vm_api::node::NodePhase;
    use gita_vm_api::operation::{OperationKind, OperationStatus};
    use gita_vm_api::quota::QuotaScope;
    use gita_vm_api::vm::VmStatus;

    fn db_owner(owner_type: &str, owner_id: i64) -> DbOwnerRef {
        DbOwnerRef {
            owner_type: owner_type.to_string(),
            owner_id,
        }
    }

    /// Every owner type round-trips through the database representation.
    #[test]
    fn the_owner_type_round_trips() {
        for (owner_type, name) in [
            (OwnerType::System, "system"),
            (OwnerType::Group, "group"),
            (OwnerType::Repo, "repo"),
        ] {
            let owner = OwnerRef {
                owner_type,
                owner_id: 7,
            };
            let db: DbOwnerRef = owner.clone().into();
            assert_eq!(db.owner_type, name);
            assert_eq!(db.owner_id, 7);
            let back: OwnerRef = db.into();
            assert_eq!(back.owner_type, owner_type);
            assert_eq!(back.owner_id, 7);
        }
    }

    /// An owner type the code does not know about is treated as a
    /// system resource rather than failing the whole row.
    #[test]
    fn an_unknown_owner_type_becomes_system() {
        let owner: OwnerRef = db_owner("tenant", 3).into();
        assert_eq!(owner.owner_type, OwnerType::System);
        assert_eq!(owner.owner_id, 3);
    }

    fn db_network(provider: &str) -> DbNetwork {
        DbNetwork {
            id: 1,
            name: "net".to_string(),
            provider: provider.to_string(),
            bridge: "br0".to_string(),
            cidr: "10.42.0.0/24".to_string(),
            gateway: "10.42.0.1".to_string(),
            dhcp_start: Some("10.42.0.10".to_string()),
            dhcp_end: None,
            mtu: 1500,
            vlan: Some(42),
            owner_type: "system".to_string(),
            owner_id: 0,
            is_default: true,
            created_at_ms: 100,
        }
    }

    #[test]
    fn every_network_provider_is_converted() {
        assert_eq!(Network::from(db_network("bridge")).provider, NetProvider::Bridge);
        assert_eq!(Network::from(db_network("macvtap")).provider, NetProvider::MacVTap);
        assert_eq!(Network::from(db_network("passt")).provider, NetProvider::Passt);
        assert_eq!(Network::from(db_network("cni")).provider, NetProvider::Cni);
    }

    /// An unknown provider falls back to the bridge, the one that needs
    /// no extra runtime.
    #[test]
    fn an_unknown_network_provider_becomes_a_bridge() {
        assert_eq!(
            Network::from(db_network("wireguard")).provider,
            NetProvider::Bridge
        );
    }

    /// The rest of the network row is carried over untouched.
    #[test]
    fn the_network_row_is_carried_over() {
        let net = Network::from(db_network("bridge"));
        assert_eq!(net.name, "net");
        assert_eq!(net.cidr, "10.42.0.0/24");
        assert_eq!(net.gateway, "10.42.0.1");
        assert_eq!(net.dhcp_start.as_deref(), Some("10.42.0.10"));
        assert_eq!(net.dhcp_end, None);
        assert_eq!(net.mtu, 1500);
        assert_eq!(net.vlan, Some(42));
        assert!(net.is_default);
        assert_eq!(net.created_at_ms, 100);
        assert_eq!(net.owner.owner_type, OwnerType::System);
    }

    fn db_lease(state: &str) -> DbIpLease {
        DbIpLease {
            id: 5,
            network_id: 1,
            ip: "10.42.0.10".to_string(),
            mac: "aa:bb:cc:dd:ee:ff".to_string(),
            vm_id: Some(9),
            node_id: None,
            state: state.to_string(),
            expires_at_ms: Some(500),
            created_at_ms: 100,
        }
    }

    #[test]
    fn every_lease_state_is_converted() {
        assert_eq!(IpLease::from(db_lease("allocated")).state, LeaseState::Allocated);
        assert_eq!(IpLease::from(db_lease("reserved")).state, LeaseState::Reserved);
        assert_eq!(IpLease::from(db_lease("released")).state, LeaseState::Released);
        assert_eq!(IpLease::from(db_lease("expired")).state, LeaseState::Expired);
    }

    /// An unknown lease state is treated as allocated: the address is
    /// held until something says otherwise.
    #[test]
    fn an_unknown_lease_state_becomes_allocated() {
        assert_eq!(
            IpLease::from(db_lease("quarantined")).state,
            LeaseState::Allocated
        );
    }

    /// The lease keeps the VM it was handed out to, and a lease that was
    /// never handed out has none.
    #[test]
    fn the_lease_keeps_its_vm() {
        let lease = IpLease::from(db_lease("allocated"));
        assert_eq!(lease.vm_id.map(|v| v.0), Some(9));
        assert_eq!(lease.node_id, None);
        assert_eq!(lease.ip, "10.42.0.10");
        assert_eq!(lease.mac, "aa:bb:cc:dd:ee:ff");
        assert_eq!(lease.expires_at_ms, Some(500));
    }

    fn db_node(phase: &str) -> DbNode {
        DbNode {
            id: 3,
            name: "node-1".to_string(),
            cell_id: Some(2),
            cpu_millis: 8000,
            memory_bytes: 16_000_000_000,
            disk_bytes: 100_000_000_000,
            capabilities_vmm: vec!["qemu".to_string()],
            capabilities_arch: vec!["x86_64".to_string()],
            nested_virt: true,
            gpu_passthrough: false,
            max_vcpus: 8,
            max_memory_bytes: 16_000_000_000,
            alloc_cpu_millis: 2000,
            alloc_memory_bytes: 1024,
            alloc_disk_bytes: 2048,
            running_vms: 1,
            phase: phase.to_string(),
            last_heartbeat_ms: 700,
            taints: serde_json::json!({}),
            labels: serde_json::json!({}),
            owner_type: "system".to_string(),
            owner_id: 0,
            conditions: serde_json::json!({}),
            created_at_ms: 100,
            updated_at_ms: 200,
        }
    }

    #[test]
    fn every_node_phase_is_converted() {
        assert_eq!(Node::from(db_node("online")).status.phase, NodePhase::Online);
        assert_eq!(
            Node::from(db_node("maintenance")).status.phase,
            NodePhase::Maintenance
        );
        assert_eq!(Node::from(db_node("draining")).status.phase, NodePhase::Draining);
        assert_eq!(Node::from(db_node("offline")).status.phase, NodePhase::Offline);
        assert_eq!(Node::from(db_node("joining")).status.phase, NodePhase::Joining);
    }

    /// A node in a phase the code does not know is treated as joining:
    /// it is not advertised as ready for work.
    #[test]
    fn an_unknown_node_phase_becomes_joining() {
        assert_eq!(
            Node::from(db_node("draining-nicely")).status.phase,
            NodePhase::Joining
        );
    }

    /// The node's capacity and its allocations are carried over, and
    /// the counts are read as unsigned.
    #[test]
    fn the_node_capacity_is_carried_over() {
        let node = Node::from(db_node("online"));
        assert_eq!(node.name, "node-1");
        assert_eq!(node.cell.map(|c| c.0), Some(2));
        assert_eq!(node.spec.cpu_millis, 8000);
        assert_eq!(node.spec.memory_bytes, 16_000_000_000);
        assert_eq!(node.spec.disk_bytes, 100_000_000_000);
        assert_eq!(node.spec.capabilities.max_vcpus, 8);
        assert!(node.spec.capabilities.nested_virt);
        assert!(!node.spec.capabilities.gpu_passthrough);
        assert_eq!(node.status.alloc_cpu_millis, 2000);
        assert_eq!(node.status.running_vms, 1);
        assert_eq!(node.status.last_heartbeat_ms, 700);
        assert_eq!(node.created_at_ms, 100);
        assert_eq!(node.updated_at_ms, 200);
    }

    /// A node that advertises a VMM the code does not know advertises
    /// no VMM at all: the control plane must not send it work it cannot
    /// run.
    #[test]
    fn an_unknown_vmm_is_not_advertised() {
        let mut row = db_node("online");
        row.capabilities_vmm = vec!["qemu".to_string(), "firecracker".to_string()];
        let node = Node::from(row);
        assert_eq!(node.spec.capabilities.vmm.len(), 1);
        assert_eq!(node.spec.capabilities.vmm[0].as_str(), "qemu");
    }

    /// The JSON columns become the typed collections they hold: the
    /// labels a map, the taints and the conditions lists.
    #[test]
    fn the_json_columns_become_their_collections() {
        let mut row = db_node("online");
        row.labels = serde_json::json!({"zone": "a"});
        row.taints = serde_json::json!([
            {"key": "disk", "value": "slow", "effect": "no_schedule"}
        ]);
        row.conditions = serde_json::json!([
            {"kind": "ready", "status": "true", "last_transition_ms": 700}
        ]);
        let node = Node::from(row);
        assert_eq!(node.labels.get("zone").map(String::as_str), Some("a"));
        assert_eq!(node.taints.len(), 1);
        assert_eq!(node.taints[0].key, "disk");
        assert_eq!(node.taints[0].value, "slow");
        assert_eq!(node.conditions.len(), 1);
        assert_eq!(node.conditions[0].last_transition_ms, 700);
    }

    /// A JSON column that does not hold the collection it should yields
    /// an empty one rather than an error: a node with a malformed label
    /// column is still a usable node.
    #[test]
    fn a_malformed_json_column_yields_an_empty_collection() {
        let mut row = db_node("online");
        row.labels = serde_json::json!("not a map");
        row.taints = serde_json::json!({"not": "a list"});
        row.conditions = serde_json::json!("not a list");
        let node = Node::from(row);
        assert!(node.labels.is_empty());
        assert!(node.taints.is_empty());
        assert!(node.conditions.is_empty());
    }

    fn db_image(kind: &str, status: &str) -> DbImage {
        DbImage {
            id: 4,
            name: "ubuntu".to_string(),
            kind: kind.to_string(),
            status: status.to_string(),
            pool_id: Some(6),
            path: "/var/lib/gita/images/ubuntu.qcow2".to_string(),
            size_bytes: 1024,
            digest: "sha256:abc".to_string(),
            format: "qcow2".to_string(),
            arch: "x86_64".to_string(),
            owner_type: "system".to_string(),
            owner_id: 0,
            tags: vec!["latest".to_string()],
            metadata: serde_json::json!({}),
            created_at_ms: 100,
            last_used_at_ms: Some(300),
        }
    }

    #[test]
    fn every_image_kind_and_status_is_converted() {
        assert_eq!(Image::from(db_image("rootfs", "ready")).kind, ImageKind::Rootfs);
        assert_eq!(Image::from(db_image("kernel", "ready")).kind, ImageKind::Kernel);
        assert_eq!(
            Image::from(db_image("cloud-init", "ready")).kind,
            ImageKind::CloudInit
        );
        assert_eq!(
            Image::from(db_image("rootfs", "ready")).status,
            ImageStatus::Ready
        );
        assert_eq!(
            Image::from(db_image("rootfs", "failed")).status,
            ImageStatus::Failed
        );
        assert_eq!(
            Image::from(db_image("rootfs", "deleting")).status,
            ImageStatus::Deleting
        );
        assert_eq!(
            Image::from(db_image("rootfs", "importing")).status,
            ImageStatus::Importing
        );
    }

    /// An unknown kind is a root filesystem and an unknown status is an
    /// image still being imported: neither is ready to use.
    #[test]
    fn an_unknown_image_kind_and_status_fall_back() {
        let image = Image::from(db_image("vhd", "pending"));
        assert_eq!(image.kind, ImageKind::Rootfs);
        assert_eq!(image.status, ImageStatus::Importing);
        assert!(!image.is_ready());
    }

    #[test]
    fn the_image_row_is_carried_over() {
        let image = Image::from(db_image("rootfs", "ready"));
        assert_eq!(image.name, "ubuntu");
        assert_eq!(image.pool_id.0, 6);
        assert_eq!(image.path, "/var/lib/gita/images/ubuntu.qcow2");
        assert_eq!(image.size_bytes, 1024);
        assert_eq!(image.digest, "sha256:abc");
        assert_eq!(image.format, "qcow2");
        assert_eq!(image.arch, "x86_64");
        assert_eq!(image.tags, vec!["latest".to_string()]);
        assert_eq!(image.last_used_at_ms, Some(300));
    }

    fn db_vm(status: &str) -> DbVm {
        DbVm {
            id: 8,
            name: "vm-1".to_string(),
            template_id: Some(2),
            owner_type: "repo".to_string(),
            owner_id: 11,
            repo_id: Some(11),
            pipeline_run_id: None,
            job_id: None,
            node_id: Some(3),
            spec: serde_json::json!({}),
            status: status.to_string(),
            generation: 2,
            observed_generation: 1,
            conditions: serde_json::json!({}),
            rootfs_path: Some("/var/lib/gita/vm/vm-1.qcow2".to_string()),
            ip_address: Some("10.42.0.10".to_string()),
            console_url: None,
            created_at_ms: 100,
            message: Some("ok".to_string()),
            expires_at_ms: None,
        }
    }

    #[test]
    fn every_vm_status_is_converted() {
        assert_eq!(Vm::from(db_vm("pending")).status, VmStatus::Pending);
        assert_eq!(
            Vm::from(db_vm("provisioning")).status,
            VmStatus::Provisioning
        );
        assert_eq!(Vm::from(db_vm("running")).status, VmStatus::Running);
        assert_eq!(Vm::from(db_vm("stopping")).status, VmStatus::Stopping);
        assert_eq!(Vm::from(db_vm("stopped")).status, VmStatus::Stopped);
        assert_eq!(Vm::from(db_vm("migrating")).status, VmStatus::Migrating);
        assert_eq!(Vm::from(db_vm("failed")).status, VmStatus::Failed);
        assert_eq!(
            Vm::from(db_vm("quarantined")).status,
            VmStatus::Quarantined
        );
    }

    /// A status the code does not know is a VM that has not started yet.
    #[test]
    fn an_unknown_vm_status_becomes_pending() {
        assert_eq!(Vm::from(db_vm("hibernating")).status, VmStatus::Pending);
    }

    #[test]
    fn the_vm_row_is_carried_over() {
        let vm = Vm::from(db_vm("running"));
        assert_eq!(vm.name, "vm-1");
        assert_eq!(vm.owner.owner_type, OwnerType::Repo);
        assert_eq!(vm.owner.owner_id, 11);
        assert_eq!(vm.node.map(|n| n.0), Some(3));
        assert_eq!(vm.generation, 2);
        assert_eq!(vm.observed_generation, 1);
        assert_eq!(vm.rootfs_path.as_deref(), Some("/var/lib/gita/vm/vm-1.qcow2"));
        assert_eq!(vm.ip_address.as_deref(), Some("10.42.0.10"));
        assert_eq!(vm.console_url, None);
        assert_eq!(vm.message.as_deref(), Some("ok"));
        assert_eq!(vm.created_at_ms, 100);
    }

    fn db_operation(kind: &str, status: &str) -> DbOperation {
        DbOperation {
            id: 12,
            kind: kind.to_string(),
            vm_id: 8,
            node_id: Some(3),
            steps: serde_json::json!([]),
            current_step: 1,
            status: status.to_string(),
            priority: 5,
            created_at_ms: 100,
            started_at_ms: Some(110),
            finished_at_ms: None,
            error: None,
        }
    }

    #[test]
    fn every_operation_kind_is_converted() {
        assert_eq!(
            Operation::from(db_operation("create_vm", "pending")).kind,
            OperationKind::CreateVm
        );
        assert_eq!(
            Operation::from(db_operation("start_vm", "pending")).kind,
            OperationKind::StartVm
        );
        assert_eq!(
            Operation::from(db_operation("stop_vm", "pending")).kind,
            OperationKind::StopVm
        );
        assert_eq!(
            Operation::from(db_operation("restart_vm", "pending")).kind,
            OperationKind::RestartVm
        );
        assert_eq!(
            Operation::from(db_operation("delete_vm", "pending")).kind,
            OperationKind::DeleteVm
        );
        assert_eq!(
            Operation::from(db_operation("migrate_vm", "pending")).kind,
            OperationKind::MigrateVm
        );
        assert_eq!(
            Operation::from(db_operation("create_snapshot", "pending")).kind,
            OperationKind::CreateSnapshot
        );
        assert_eq!(
            Operation::from(db_operation("restore_snapshot", "pending")).kind,
            OperationKind::RestoreSnapshot
        );
        assert_eq!(
            Operation::from(db_operation("resize_disk", "pending")).kind,
            OperationKind::ResizeDisk
        );
        assert_eq!(
            Operation::from(db_operation("hotplug_resources", "pending")).kind,
            OperationKind::HotplugResources
        );
        assert_eq!(
            Operation::from(db_operation("update_network", "pending")).kind,
            OperationKind::UpdateNetwork
        );
        assert_eq!(
            Operation::from(db_operation("inject_user_data", "pending")).kind,
            OperationKind::InjectUserData
        );
    }

    /// An operation the code does not know is a create: the two are the
    /// only ones that must not be skipped.
    #[test]
    fn an_unknown_operation_kind_becomes_create() {
        assert_eq!(
            Operation::from(db_operation("teleport_vm", "pending")).kind,
            OperationKind::CreateVm
        );
    }

    #[test]
    fn every_operation_status_is_converted() {
        assert_eq!(
            Operation::from(db_operation("start_vm", "pending")).status,
            OperationStatus::Pending
        );
        assert_eq!(
            Operation::from(db_operation("start_vm", "running")).status,
            OperationStatus::Running
        );
        assert_eq!(
            Operation::from(db_operation("start_vm", "succeeded")).status,
            OperationStatus::Succeeded
        );
        assert_eq!(
            Operation::from(db_operation("start_vm", "failed")).status,
            OperationStatus::Failed
        );
        assert_eq!(
            Operation::from(db_operation("start_vm", "cancelling")).status,
            OperationStatus::Cancelling
        );
        assert_eq!(
            Operation::from(db_operation("start_vm", "cancelled")).status,
            OperationStatus::Cancelled
        );
    }

    /// An unknown status is a pending operation: it has not run yet.
    #[test]
    fn an_unknown_operation_status_becomes_pending() {
        assert_eq!(
            Operation::from(db_operation("start_vm", "paused")).status,
            OperationStatus::Pending
        );
    }

    #[test]
    fn the_operation_row_is_carried_over() {
        let op = Operation::from(db_operation("start_vm", "running"));
        assert_eq!(op.vm_id.0, 8);
        assert_eq!(op.node_id.map(|n| n.0), Some(3));
        assert_eq!(op.current_step, 1);
        assert_eq!(op.priority, 5);
        assert_eq!(op.created_at_ms, 100);
        assert_eq!(op.started_at_ms, Some(110));
        assert_eq!(op.finished_at_ms, None);
        assert_eq!(op.error, None);
    }

    fn db_quota(scope: &str) -> DbQuota {
        DbQuota {
            id: 1,
            scope: scope.to_string(),
            scope_id: 42,
            max_concurrent_vms: 10,
            max_cpu_millis: 8000,
            max_memory_bytes: 16_000_000_000,
            max_disk_bytes: 100_000_000_000,
            max_vm_lifetime_ms: 3_600_000,
            created_at_ms: 100,
            updated_at_ms: 200,
        }
    }

    #[test]
    fn every_quota_scope_is_converted() {
        assert_eq!(Quota::from(db_quota("system")).scope, QuotaScope::System);
        assert_eq!(Quota::from(db_quota("group")).scope, QuotaScope::Group);
        assert_eq!(Quota::from(db_quota("repo")).scope, QuotaScope::Repo);
        assert_eq!(Quota::from(db_quota("user")).scope, QuotaScope::User);
    }

    /// An unknown scope is a system quota: the limit that applies to
    /// everybody rather than to nobody.
    #[test]
    fn an_unknown_quota_scope_becomes_system() {
        assert_eq!(Quota::from(db_quota("tenant")).scope, QuotaScope::System);
    }

    #[test]
    fn the_quota_row_is_carried_over() {
        let quota = Quota::from(db_quota("group"));
        assert_eq!(quota.scope_id, 42);
        assert_eq!(quota.max_concurrent_vms, 10);
        assert_eq!(quota.max_cpu_millis, 8000);
        assert_eq!(quota.max_memory_bytes, 16_000_000_000);
        assert_eq!(quota.max_disk_bytes, 100_000_000_000);
        assert_eq!(quota.max_vm_lifetime_ms, 3_600_000);
        assert_eq!(quota.created_at_ms, 100);
        assert_eq!(quota.updated_at_ms, 200);
    }
}
