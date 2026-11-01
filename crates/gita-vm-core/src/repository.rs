//! Repository trait：持久化契约
//!
//! 所有数据库操作定义为 trait，实现层（gita-vm-store）用 sqlx/Postgres 实现。
//!
//! 每个方法都有默认实现（返回 `Error::Storage("not implemented")`），
//! 这样测试替身只需覆盖自己关心的方法。

use crate::{Error, Result};
use async_trait::async_trait;
use gita_vm_api::ids::{ImageId, NetworkId, NodeId, OperationId, TemplateId, VmId};
use gita_vm_api::network::IpLease;
use gita_vm_api::node::Node;
use gita_vm_api::operation::Operation;
use gita_vm_api::quota::{Quota, QuotaScope, QuotaUsage};
use gita_vm_api::template::VmTemplate;
use gita_vm_api::vm::{Vm, VmLinkage};

#[async_trait]
pub trait Repository: Send + Sync {
    // ===== VM =====
    async fn create_vm(&self, _vm: &Vm) -> Result<()> {
        Err(unimpl())
    }
    async fn get_vm(&self, _id: VmId) -> Result<Option<Vm>> {
        Err(unimpl())
    }
    async fn update_vm(&self, _vm: &Vm) -> Result<()> {
        Err(unimpl())
    }
    async fn delete_vm(&self, _id: VmId) -> Result<()> {
        Err(unimpl())
    }
    async fn list_vms(&self, _owner: Option<gita_vm_api::ids::OwnerRef>) -> Result<Vec<Vm>> {
        Err(unimpl())
    }
    async fn list_vms_by_node(&self, _node_id: NodeId) -> Result<Vec<Vm>> {
        Err(unimpl())
    }

    /// 创建一台**带 gita 关联**的 VM，并返回它的 id。
    ///
    /// CI 场景必须走这里：普通 `create_vm` 不写 `repo_id` /
    /// `pipeline_run_id` / `job_id`，那台 VM 就与任何 job 无关——控制面
    /// 不知道该不该回收它，Web UI 也显示不出来。返回值必须是真实 id：
    /// 调用方（runner）要靠它回收这台 VM。
    async fn create_linked_vm(&self, _vm: &Vm, _linkage: VmLinkage) -> Result<VmId> {
        Err(unimpl())
    }

    /// 读取一台 VM 与 gita 侧的关联。
    async fn get_vm_linkage(&self, _id: VmId) -> Result<Option<VmLinkage>> {
        Err(unimpl())
    }

    // ===== Template =====
    async fn create_template(&self, _tmpl: &VmTemplate) -> Result<()> {
        Err(unimpl())
    }
    async fn get_template(&self, _id: TemplateId) -> Result<Option<VmTemplate>> {
        Err(unimpl())
    }
    async fn update_template(&self, _tmpl: &VmTemplate) -> Result<()> {
        Err(unimpl())
    }
    async fn delete_template(&self, _id: TemplateId) -> Result<()> {
        Err(unimpl())
    }
    async fn list_templates(
        &self,
        _owner: Option<gita_vm_api::ids::OwnerRef>,
    ) -> Result<Vec<VmTemplate>> {
        Err(unimpl())
    }

    // ===== Node =====
    async fn create_node(&self, _node: &Node) -> Result<()> {
        Err(unimpl())
    }
    async fn get_node(&self, _id: NodeId) -> Result<Option<Node>> {
        Err(unimpl())
    }
    async fn update_node(&self, _node: &Node) -> Result<()> {
        Err(unimpl())
    }
    async fn delete_node(&self, _id: NodeId) -> Result<()> {
        Err(unimpl())
    }
    async fn list_nodes(&self, _owner: Option<gita_vm_api::ids::OwnerRef>) -> Result<Vec<Node>> {
        Err(unimpl())
    }

    // ===== Image =====
    async fn create_image(&self, _image: &gita_vm_api::Image) -> Result<()> {
        Err(unimpl())
    }
    async fn get_image(&self, _id: ImageId) -> Result<Option<gita_vm_api::Image>> {
        Err(unimpl())
    }
    async fn update_image(&self, _image: &gita_vm_api::Image) -> Result<()> {
        Err(unimpl())
    }
    async fn delete_image(&self, _id: ImageId) -> Result<()> {
        Err(unimpl())
    }
    async fn list_images(
        &self,
        _owner: Option<gita_vm_api::ids::OwnerRef>,
    ) -> Result<Vec<gita_vm_api::Image>> {
        Err(unimpl())
    }

    // ===== Network =====
    async fn create_network(&self, _net: &gita_vm_api::network::Network) -> Result<()> {
        Err(unimpl())
    }
    async fn get_network(&self, _id: NetworkId) -> Result<Option<gita_vm_api::network::Network>> {
        Err(unimpl())
    }
    async fn update_network(&self, _net: &gita_vm_api::network::Network) -> Result<()> {
        Err(unimpl())
    }
    async fn delete_network(&self, _id: NetworkId) -> Result<()> {
        Err(unimpl())
    }
    async fn list_networks(
        &self,
        _owner: Option<gita_vm_api::ids::OwnerRef>,
    ) -> Result<Vec<gita_vm_api::network::Network>> {
        Err(unimpl())
    }
    async fn get_default_network(&self) -> Result<Option<gita_vm_api::network::Network>> {
        Err(unimpl())
    }

    // ===== IP Lease =====
    async fn create_ip_lease(&self, _lease: &IpLease) -> Result<i64> {
        Err(unimpl())
    }
    async fn get_ip_lease_by_ip(&self, _ip: &str) -> Result<Option<IpLease>> {
        Err(unimpl())
    }
    async fn get_ip_lease_by_vm(&self, _vm_id: VmId) -> Result<Option<IpLease>> {
        Err(unimpl())
    }
    async fn get_ip_lease_by_id(&self, _id: i64) -> Result<Option<IpLease>> {
        Err(unimpl())
    }
    async fn update_ip_lease(&self, _lease: &IpLease) -> Result<()> {
        Err(unimpl())
    }
    async fn release_ip_lease_for_vm(&self, _vm_id: VmId) -> Result<()> {
        Err(unimpl())
    }
    async fn list_ip_leases(&self, _network_id: NetworkId) -> Result<Vec<IpLease>> {
        Err(unimpl())
    }

    // ===== Operation =====
    async fn create_operation(&self, _op: &Operation) -> Result<()> {
        Err(unimpl())
    }
    async fn get_operation(&self, _id: OperationId) -> Result<Option<Operation>> {
        Err(unimpl())
    }
    async fn update_operation(&self, _op: &Operation) -> Result<()> {
        Err(unimpl())
    }
    async fn list_operations(&self, _node_id: Option<NodeId>) -> Result<Vec<Operation>> {
        Err(unimpl())
    }
    async fn list_pending_operations(
        &self,
        _node_id: Option<NodeId>,
        _limit: usize,
    ) -> Result<Vec<Operation>> {
        Err(unimpl())
    }

    // ===== Quota =====
    async fn create_quota(&self, _quota: &Quota) -> Result<()> {
        Err(unimpl())
    }
    async fn get_quota(&self, _scope: QuotaScope, _scope_id: i64) -> Result<Option<Quota>> {
        Err(unimpl())
    }
    async fn update_quota(&self, _quota: &Quota) -> Result<()> {
        Err(unimpl())
    }
    async fn delete_quota(&self, _scope: QuotaScope, _scope_id: i64) -> Result<()> {
        Err(unimpl())
    }
    async fn list_quotas(&self) -> Result<Vec<Quota>> {
        Err(unimpl())
    }
    async fn get_quota_usage(&self, _scope: QuotaScope, _scope_id: i64) -> Result<QuotaUsage> {
        Err(unimpl())
    }

    // ===== Event =====
    async fn append_event(&self, _event: &gita_vm_api::Event) -> Result<()> {
        Err(unimpl())
    }
    async fn list_events(
        &self,
        _repo_id: Option<i64>,
        _event_type: Option<&str>,
        _limit: i64,
        _offset: i64,
    ) -> Result<Vec<gita_vm_api::Event>> {
        Err(unimpl())
    }
}

fn unimpl() -> Error {
    Error::Storage("repository method not implemented for this backend".into())
}
