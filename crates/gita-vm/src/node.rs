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
#[cfg(test)]
mod tests {
    use super::*;
    use gita_vm_api::ids::OwnerRef;
    use gita_vm_api::node::NodeCapabilities;
    use std::collections::BTreeMap;

    fn url() -> String {
        std::env::var("GITA_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://gita:gita@127.0.0.1:55432/gita".to_string())
    }

    async fn manager() -> (gita_db::Db, NodeManager) {
        let db = gita_db::new_test_db(&url()).await;
        let store = Arc::new(VmStore::new(db.pool().clone()));
        (db, NodeManager::new(store))
    }

    fn spec() -> NodeSpec {
        NodeSpec {
            cpu_millis: 8000,
            memory_bytes: 16_000_000_000,
            disk_bytes: 100_000_000_000,
            capabilities: NodeCapabilities {
                vmm: vec![gita_vm_api::node::VmmKind::Qemu],
                arch: vec!["x86_64".to_string()],
                nested_virt: false,
                gpu_passthrough: false,
                max_vcpus: 8,
                max_memory_bytes: 16_000_000_000,
            },
        }
    }

    fn status(phase: NodePhase) -> NodeStatus {
        NodeStatus {
            alloc_cpu_millis: 0,
            alloc_memory_bytes: 0,
            alloc_disk_bytes: 0,
            running_vms: 0,
            phase,
            last_heartbeat_ms: 0,
        }
    }

    /// A node that registers can be found again by its name.
    #[tokio::test]
    async fn a_registered_node_can_be_found() {
        let (db, mgr) = manager().await;
        let id = mgr
            .register_node("node-1", spec(), OwnerRef::system(), BTreeMap::new())
            .await
            .unwrap();
        assert!(id.0 > 0, "the node must get an id");

        let node = mgr
            .store
            .get_node(id)
            .await
            .unwrap()
            .expect("the node must be there");
        assert_eq!(node.name, "node-1");
        assert_eq!(node.status.phase, NodePhase::Joining);
        assert_eq!(node.spec.cpu_millis, 8000);
        let _ = db;
    }

    /// The labels a node registers with are kept.
    #[tokio::test]
    async fn the_labels_of_a_node_are_kept() {
        let (_db, mgr) = manager().await;
        let mut labels = BTreeMap::new();
        labels.insert("zone".to_string(), "a".to_string());
        let id = mgr
            .register_node("node-1", spec(), OwnerRef::system(), labels)
            .await
            .unwrap();
        let node = mgr.store.get_node(id).await.unwrap().unwrap();
        assert_eq!(node.labels.get("zone").map(String::as_str), Some("a"));
    }

    /// Two nodes cannot share a name.
    #[tokio::test]
    async fn a_node_name_can_only_be_taken_once() {
        let (_db, mgr) = manager().await;
        mgr.register_node("node-1", spec(), OwnerRef::system(), BTreeMap::new())
            .await
            .unwrap();
        let err = mgr
            .register_node("node-1", spec(), OwnerRef::system(), BTreeMap::new())
            .await
            .expect_err("the second registration must fail");
        assert!(matches!(err, Error::AlreadyExists(_)), "got {err:?}");
    }

    /// A heartbeat puts the node online and refreshes its timestamp.
    #[tokio::test]
    async fn a_heartbeat_puts_the_node_online() {
        let (_db, mgr) = manager().await;
        let id = mgr
            .register_node("node-1", spec(), OwnerRef::system(), BTreeMap::new())
            .await
            .unwrap();
        mgr.heartbeat(id, status(NodePhase::Joining)).await.unwrap();

        let node = mgr.store.get_node(id).await.unwrap().unwrap();
        assert_eq!(node.status.phase, NodePhase::Online);
        assert!(node.status.last_heartbeat_ms > 0);
    }

    /// A heartbeat for a node that does not exist is refused.
    #[tokio::test]
    async fn a_heartbeat_for_an_unknown_node_is_refused() {
        let (_db, mgr) = manager().await;
        let err = mgr
            .heartbeat(NodeId(999), status(NodePhase::Online))
            .await
            .expect_err("the heartbeat must fail");
        assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
    }

    /// A node that stopped sending heartbeats is taken offline.
    #[tokio::test]
    async fn a_node_that_stops_heartbeating_goes_offline() {
        let (_db, mgr) = manager().await;
        let id = mgr
            .register_node("node-1", spec(), OwnerRef::system(), BTreeMap::new())
            .await
            .unwrap();
        mgr.heartbeat(id, status(NodePhase::Online)).await.unwrap();

        // Move the heartbeat into the past.
        let mut node = mgr.store.get_node(id).await.unwrap().unwrap();
        node.status.last_heartbeat_ms = gita_vm_core::now_ms() - 60_000;
        mgr.store.update_node(&node).await.unwrap();

        let offline = mgr.detect_offline_nodes().await.unwrap();
        assert_eq!(offline, vec![id], "got {offline:?}");

        let node = mgr.store.get_node(id).await.unwrap().unwrap();
        assert_eq!(node.status.phase, NodePhase::Offline);
    }

    /// A node whose heartbeat is still within the timeout is online.
    ///
    /// The margin keeps the test from depending on how long the
    /// database round trip takes: the node is a second short of the
    /// timeout, which no round trip can eat.
    #[tokio::test]
    async fn a_node_within_the_timeout_is_still_online() {
        let (_db, mgr) = manager().await;
        let mgr = mgr.with_heartbeat_timeout(30_000);
        let id = mgr
            .register_node("node-1", spec(), OwnerRef::system(), BTreeMap::new())
            .await
            .unwrap();

        let mut node = mgr.store.get_node(id).await.unwrap().unwrap();
        node.status.phase = NodePhase::Online;
        node.status.last_heartbeat_ms = gita_vm_core::now_ms() - 29_000;
        mgr.store.update_node(&node).await.unwrap();

        let offline = mgr.detect_offline_nodes().await.unwrap();
        assert!(offline.is_empty(), "got {offline:?}");
    }

    /// A node that is already offline, in maintenance or draining is
    /// left alone: the detection only concerns nodes that are up.
    #[tokio::test]
    async fn a_node_that_is_not_up_is_left_alone() {
        let (_db, mgr) = manager().await;
        let id = mgr
            .register_node("node-1", spec(), OwnerRef::system(), BTreeMap::new())
            .await
            .unwrap();

        let mut node = mgr.store.get_node(id).await.unwrap().unwrap();
        node.status.phase = NodePhase::Maintenance;
        node.status.last_heartbeat_ms = gita_vm_core::now_ms() - 60_000;
        mgr.store.update_node(&node).await.unwrap();

        let offline = mgr.detect_offline_nodes().await.unwrap();
        assert!(offline.is_empty(), "got {offline:?}");
        let node = mgr.store.get_node(id).await.unwrap().unwrap();
        assert_eq!(node.status.phase, NodePhase::Maintenance);
    }

    /// A node in maintenance mode does not take new work; taking it out
    /// of maintenance puts it back online.
    #[tokio::test]
    async fn a_node_can_be_put_into_and_out_of_maintenance() {
        let (_db, mgr) = manager().await;
        let id = mgr
            .register_node("node-1", spec(), OwnerRef::system(), BTreeMap::new())
            .await
            .unwrap();

        mgr.set_maintenance(id, true).await.unwrap();
        let node = mgr.store.get_node(id).await.unwrap().unwrap();
        assert_eq!(node.status.phase, NodePhase::Maintenance);

        mgr.set_maintenance(id, false).await.unwrap();
        let node = mgr.store.get_node(id).await.unwrap().unwrap();
        assert_eq!(node.status.phase, NodePhase::Online);
    }

    /// Draining a node tells it to give up its work.
    #[tokio::test]
    async fn a_node_can_be_drained() {
        let (_db, mgr) = manager().await;
        let id = mgr
            .register_node("node-1", spec(), OwnerRef::system(), BTreeMap::new())
            .await
            .unwrap();
        mgr.drain(id).await.unwrap();
        let node = mgr.store.get_node(id).await.unwrap().unwrap();
        assert_eq!(node.status.phase, NodePhase::Draining);
    }

    /// Acting on a node that does not exist is refused.
    #[tokio::test]
    async fn acting_on_an_unknown_node_is_refused() {
        let (_db, mgr) = manager().await;
        for err in [
            mgr.set_maintenance(NodeId(999), true).await.err(),
            mgr.drain(NodeId(999)).await.err(),
        ] {
            let err = err.expect("the call must fail");
            assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
        }
    }
}
