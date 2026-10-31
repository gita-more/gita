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