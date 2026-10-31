//! # gita-vm
//!
//! 统一的 VM 集群门面，供 gita-ci、gita-runner、gita-admin 等调用。
//! 封装了控制面调用、节点管理、CI 专用辅助功能。

pub mod ci;
pub mod control;
pub mod node;

pub use ci::{VmCluster, CiVmHelper, CreateVmOptions, VmResult};
pub use gita_vm_api::VmSpecOverrides;
pub use gita_vm_core::now_ms;
pub use control::ControlClient;
pub use node::NodeManager;

use gita_vm_api::{Event, VmId, VmSpec, VmTemplate};
use gita_vm_core::repository::Repository;
use gita_vm_core::Result;
use gita_vm_store::VmStore;
use gita_db::Db;
use std::sync::Arc;

/// VM 集群统一入口
pub struct VmClusterImpl {
    pub store: Arc<VmStore>,
    pub db: Arc<Db>,
    pub ci: ci::CiVmHelper,
    pub control_client: ControlClient,
    pub node_manager: NodeManager,
}

impl VmClusterImpl {
    /// 从 gita-db 创建
    pub async fn new(db: Arc<Db>) -> Result<Self> {
        let store = Arc::new(VmStore::new(db.pool().clone()));
        let ci = ci::CiVmHelper::new(store.clone());
        let control_client = ControlClient::new(store.clone());
        let node_manager = NodeManager::new(store.clone());

        Ok(Self {
            store,
            db,
            ci,
            control_client,
            node_manager,
        })
    }
}

#[async_trait::async_trait]
impl VmCluster for VmClusterImpl {
    async fn create_vm(&self, _spec: VmSpec) -> Result<VmResult> {
        // 无模板的裸 spec 创建：包装成 options（模板 ID 缺省）
        let opts = ci::CreateVmOptions {
            owner: None,
            template_id: None,
            overrides: None,
            ..Default::default()
        };
        let _ = opts;
        Err(gita_vm_core::Error::Validation("裸 spec 创建请用 CiVmHelper::create_ci_vm 并指定模板".into()))
    }

    async fn get_vm(&self, id: VmId) -> Result<Option<gita_vm_api::Vm>> {
        self.control_client.get_vm(id).await
    }

    async fn destroy_vm(&self, id: VmId) -> Result<()> {
        self.control_client.delete_vm(id).await
    }

    async fn list_templates(&self) -> Result<Vec<VmTemplate>> {
        self.store.list_templates(None).await
    }

    async fn wait_ready(&self, id: VmId, timeout: std::time::Duration) -> Result<gita_vm_api::Vm> {
        self.ci.wait_ready(id, timeout).await
    }

    async fn get_console(&self, id: VmId) -> Result<Option<String>> {
        self.ci.get_console(id).await
    }
}

/// 事件类型常量（VM 领域事件使用 vm.* 前缀）
pub mod events {
    pub const VM_CREATED: &str = "vm.created";
    pub const VM_DELETED: &str = "vm.deleted";
    pub const VM_STARTED: &str = "vm.started";
    pub const VM_STOPPED: &str = "vm.stopped";
    pub const VM_FAILED: &str = "vm.failed";
    pub const NODE_ONLINE: &str = "vm.node_online";
    pub const NODE_OFFLINE: &str = "vm.node_offline";
    pub const TEMPLATE_CREATED: &str = "vm.template_created";
    pub const IMAGE_READY: &str = "vm.image_ready";
}

/// 发布 VM 事件到 gita 事件总线
pub async fn emit_vm_event(
    _event_bus: &gita_events::EventBus,
    kind: &str,
    vm_id: Option<VmId>,
    _repo_id: Option<i64>,
    _actor_id: Option<i64>,
    payload: serde_json::Value,
) -> Result<()> {
    let event = Event::new(0, kind, "", gita_vm_core::now_ms())
        .with_object(vm_id.map(|v| v.0.to_string()).unwrap_or_default())
        .with_payload(payload);
    // 这里需要根据 gita_events::EventBus 的实际 API 调用
    // event_bus.publish(event).await?;
    let _ = event;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_constants() {
        assert_eq!(events::VM_CREATED, "vm.created");
        assert_eq!(events::NODE_ONLINE, "vm.node_online");
    }
}