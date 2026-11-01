//! 控制面客户端：给 gita-vm 门面用的进程内调用封装

use gita_vm_api::ids::{NodeId, VmId};
use gita_vm_api::node::Node;
use gita_vm_api::vm::Vm;
use gita_vm_core::repository::Repository;
use gita_vm_core::{Error, Result};
use gita_vm_store::VmStore;
use std::sync::Arc;

/// 控制面客户端（进程内调用，不经过 HTTP）
pub struct ControlClient {
    store: Arc<VmStore>,
}

impl ControlClient {
    pub fn new(store: Arc<VmStore>) -> Self {
        Self { store }
    }

    /// 获取 VM
    pub async fn get_vm(&self, vm_id: VmId) -> Result<Option<Vm>> {
        self.store.get_vm(vm_id).await
    }

    /// 删除 VM（触发 DeleteVm 操作）
    pub async fn delete_vm(&self, vm_id: VmId) -> Result<()> {
        let vm = self
            .store
            .get_vm(vm_id)
            .await?
            .ok_or_else(|| Error::not_found(format!("VM {vm_id}")))?;

        // 状态机：只有可删除状态才能删除
        if !gita_vm_core::state::can_delete(vm.status) {
            // 对于运行中的 VM，先标记 Stopping，由 reconcile 完成实际删除
            // 简化：直接标记 failed？不，这里应该走状态机
            return Err(Error::InvalidTransition(format!(
                "VM {vm_id} 处于 {:?} 状态，不能直接删除",
                vm.status
            )));
        }

        self.store.delete_vm(vm_id).await
    }

    /// 获取节点列表
    pub async fn list_nodes(&self) -> Result<Vec<Node>> {
        self.store.list_nodes(None).await
    }

    /// 获取节点上的 VM 列表
    pub async fn list_node_vms(&self, node_id: NodeId) -> Result<Vec<Vm>> {
        self.store.list_vms_by_node(node_id).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use gita_vm_api::ids::OwnerRef;
    use gita_vm_api::vm::{Vm, VmSpec, VmStatus};

    fn url() -> String {
        std::env::var("GITA_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://gita:gita@127.0.0.1:55432/gita".to_string())
    }

    async fn client() -> (gita_db::Db, ControlClient) {
        let db = gita_db::new_test_db(&url()).await;
        let store = Arc::new(VmStore::new(db.pool().clone()));
        (db, ControlClient::new(store))
    }

    fn a_vm(status: VmStatus) -> Vm {
        Vm {
            id: VmId(0),
            name: "vm-1".to_string(),
            owner: OwnerRef::system(),
            cell: None,
            node: None,
            spec: VmSpec::default(),
            status,
            generation: 1,
            observed_generation: 1,
            conditions: vec![],
            rootfs_path: None,
            ip_address: None,
            console_url: None,
            created_at_ms: 1000,
            message: None,
            expires_at_ms: None,
        }
    }

    /// A VM that is running cannot be deleted: it has to be stopped
    /// first.
    #[tokio::test]
    async fn a_running_vm_cannot_be_deleted() {
        let (db, ctl) = client().await;
        let store = VmStore::new(db.pool().clone());
        store.create_vm(&a_vm(VmStatus::Running)).await.unwrap();
        let id = store.list_vms(None).await.unwrap()[0].id;

        let err = ctl.delete_vm(id).await.expect_err("the delete must fail");
        assert!(matches!(err, Error::InvalidTransition(_)), "got {err:?}");

        // The VM is still there.
        assert!(ctl.get_vm(id).await.unwrap().is_some());
    }

    /// A VM that is stopped can be deleted.
    #[tokio::test]
    async fn a_stopped_vm_can_be_deleted() {
        let (db, ctl) = client().await;
        let store = VmStore::new(db.pool().clone());
        store.create_vm(&a_vm(VmStatus::Stopped)).await.unwrap();
        let id = store.list_vms(None).await.unwrap()[0].id;

        ctl.delete_vm(id).await.unwrap();
        assert!(ctl.get_vm(id).await.unwrap().is_none());
    }

    /// A VM that failed can be deleted: it will never run again.
    #[tokio::test]
    async fn a_failed_vm_can_be_deleted() {
        let (db, ctl) = client().await;
        let store = VmStore::new(db.pool().clone());
        store.create_vm(&a_vm(VmStatus::Failed)).await.unwrap();
        let id = store.list_vms(None).await.unwrap()[0].id;

        ctl.delete_vm(id).await.unwrap();
        assert!(ctl.get_vm(id).await.unwrap().is_none());
    }

    /// Deleting a VM that is not there is refused, not silently
    /// ignored: the caller asked about a VM that does not exist.
    #[tokio::test]
    async fn deleting_an_unknown_vm_is_refused() {
        let (_db, ctl) = client().await;
        let err = ctl
            .delete_vm(VmId(999))
            .await
            .expect_err("the delete must fail");
        assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
    }

    /// The nodes and their VMs are listed.
    #[tokio::test]
    async fn the_nodes_and_their_vms_are_listed() {
        let (db, ctl) = client().await;
        let store = VmStore::new(db.pool().clone());
        assert!(ctl.list_nodes().await.unwrap().is_empty());

        let node = gita_vm_api::node::Node {
            id: NodeId(0),
            name: "node-1".to_string(),
            cell: None,
            spec: gita_vm_api::node::NodeSpec {
                cpu_millis: 8000,
                memory_bytes: 16_000_000_000,
                disk_bytes: 100_000_000_000,
                capabilities: gita_vm_api::node::NodeCapabilities {
                    vmm: vec![],
                    arch: vec![],
                    nested_virt: false,
                    gpu_passthrough: false,
                    max_vcpus: 8,
                    max_memory_bytes: 16_000_000_000,
                },
            },
            status: gita_vm_api::node::NodeStatus::default(),
            taints: vec![],
            labels: Default::default(),
            owner: OwnerRef::system(),
            conditions: vec![],
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };
        store.create_node(&node).await.unwrap();
        let node_id = store.list_nodes(None).await.unwrap()[0].id;

        let mut vm = a_vm(VmStatus::Running);
        vm.node = Some(node_id);
        store.create_vm(&vm).await.unwrap();

        assert_eq!(ctl.list_nodes().await.unwrap().len(), 1);
        assert_eq!(ctl.list_node_vms(node_id).await.unwrap().len(), 1);
        assert!(ctl.list_node_vms(NodeId(999)).await.unwrap().is_empty());
    }
}
