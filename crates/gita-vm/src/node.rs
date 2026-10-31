//! 节点管理：注册、心跳、下线检测

use gita_vm_api::ids::NodeId;
use gita_vm_api::node::{Node, NodePhase, NodeSpec, NodeStatus};
use gita_vm_core::repository::Repository;
use gita_vm_core::{Error, Result};
use gita_vm_store::VmStore;
use std::sync::Arc;

/// 节点管理器
pub struct NodeManager {
    store: Arc<VmStore>,
    /// 心跳超时（毫秒）；超过此时间无心跳视为下线
    heartbeat_timeout_ms: i64,
}

impl NodeManager {
    pub fn new(store: Arc<VmStore>) -> Self {
        Self {
            store,
            heartbeat_timeout_ms: 30_000,
        }
    }

    pub fn with_heartbeat_timeout(mut self, timeout_ms: i64) -> Self {
        self.heartbeat_timeout_ms = timeout_ms;
        self
    }

    /// 注册节点
    pub async fn register_node(
        &self,
        name: &str,
        spec: NodeSpec,
        owner: gita_vm_api::ids::OwnerRef,
        labels: std::collections::BTreeMap<String, String>,
    ) -> Result<NodeId> {
        // 检查名字冲突
        let existing = self.store.list_nodes(None).await?;
        if existing.iter().any(|n| n.name == name) {
            return Err(Error::already_exists(format!("节点 {name}")));
        }

        let now = gita_vm_core::now_ms();
        let node = Node {
            id: NodeId(0),
            name: name.to_string(),
            cell: None,
            spec,
            status: NodeStatus {
                alloc_cpu_millis: 0,
                alloc_memory_bytes: 0,
                alloc_disk_bytes: 0,
                running_vms: 0,
                phase: NodePhase::Joining,
                last_heartbeat_ms: now,
            },
            taints: vec![],
            labels,
            owner,
            conditions: vec![],
            created_at_ms: now,
            updated_at_ms: now,
        };

        self.store.create_node(&node).await?;

        // 获取分配的 ID（重新查询）
        let nodes = self.store.list_nodes(None).await?;
        nodes
            .iter()
            .find(|n| n.name == name)
            .map(|n| n.id)
            .ok_or_else(|| Error::Storage("节点创建后未找到".into()))
    }

    /// 节点心跳更新
    pub async fn heartbeat(&self, node_id: NodeId, status: NodeStatus) -> Result<()> {
        let mut node = self
            .store
            .get_node(node_id)
            .await?
            .ok_or_else(|| Error::not_found(format!("节点 {node_id}")))?;

        node.status = status;
        node.status.last_heartbeat_ms = gita_vm_core::now_ms();
        node.status.phase = NodePhase::Online;
        self.store.update_node(&node).await
    }

    /// 检测下线节点（由后台循环调用）
    pub async fn detect_offline_nodes(&self) -> Result<Vec<NodeId>> {
        let nodes = self.store.list_nodes(None).await?;
        let now = gita_vm_core::now_ms();
        let mut offline = Vec::new();

        for node in nodes {
            if matches!(node.status.phase, NodePhase::Online | NodePhase::Joining)
                && now - node.status.last_heartbeat_ms > self.heartbeat_timeout_ms
            {
                let mut n = node;
                n.status.phase = NodePhase::Offline;
                self.store.update_node(&n).await?;
                offline.push(n.id);
            }
        }
        Ok(offline)
    }

    /// 节点进入维护模式（不再调度新 VM）
    pub async fn set_maintenance(&self, node_id: NodeId, enable: bool) -> Result<()> {
        let mut node = self
            .store
            .get_node(node_id)
            .await?
            .ok_or_else(|| Error::not_found(format!("节点 {node_id}")))?;

        node.status.phase = if enable {
            NodePhase::Maintenance
        } else {
            NodePhase::Online
        };
        self.store.update_node(&node).await
    }

    /// 节点排水（驱逐所有 VM 准备下线）
    pub async fn drain(&self, node_id: NodeId) -> Result<()> {
        let mut node = self
            .store
            .get_node(node_id)
            .await?
            .ok_or_else(|| Error::not_found(format!("节点 {node_id}")))?;

        node.status.phase = NodePhase::Draining;
        self.store.update_node(&node).await
        // 实际驱逐由 reconcile 循环执行
    }
}