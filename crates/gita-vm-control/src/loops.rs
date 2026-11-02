//! 后台协调循环：reconcile（调度收敛）、node-watch（节点健康）、vm-gc（过期回收）

use crate::state::AppState;
use std::time::Duration;

/// 启动所有后台循环
pub fn start_loops(state: AppState) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::new();

    handles.push(tokio::spawn(reconcile_loop(state.clone())));
    handles.push(tokio::spawn(node_watch_loop(state.clone())));
    handles.push(tokio::spawn(vm_gc_loop(state.clone())));

    handles
}

/// reconcile 循环：把 Pending 的 VM 调度到节点，把待执行操作分发给节点
async fn reconcile_loop(state: AppState) {
    let mut ticker = tokio::time::interval(state.config.reconcile_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        if let Err(e) = reconcile_once(&state).await {
            tracing::warn!(error = %e, "reconcile loop error");
        }
    }
}

/// 单次 reconcile
///
/// 收敛调度：每一台待调度的 VM 都该落到一个节点上。它是循环的一圈，
/// 也是循环里唯一做事的地方——测试要能单独跑它。
pub async fn reconcile_once(state: &AppState) -> anyhow::Result<()> {
    // 1. 找出 Pending 状态的 VM
    let vms = state.repo.list_vms(None).await?;
    let pending: Vec<_> = vms.iter().filter(|v| matches!(v.status, gita_vm_api::vm::VmStatus::Pending)).collect();

    if pending.is_empty() {
        return Ok(());
    }

    // 2. 列出可用节点
    let nodes = state.repo.list_nodes(None).await?;
    let scheduler = gita_vm_core::scheduler::Scheduler {
        overload_ratio: state.config.overload_ratio,
        system_reserved: gita_vm_api::node::Resources::zero(),
    };

    for vm in pending {
        let placement = match scheduler.place(&vm.spec, &nodes) {
            Ok(p) => p,
            Err(reason) => {
                tracing::info!(vm = %vm.id, %reason, "no placement for vm yet");
                continue;
            }
        };

        // 3. 更新 VM 状态 → Provisioning，记录节点
        let mut updated = vm.clone();
        updated.node = Some(gita_vm_api::ids::NodeId(placement.node_id));
        if let Ok(new_vm) = gita_vm_core::state::apply_event(&updated, gita_vm_core::state::VmEvent::Scheduled) {
            state.repo.update_vm(&new_vm).await?;
            state.emit(
                "vm.scheduled",
                Some(vm.id.0.to_string()),
                format!("VM {} scheduled to node {}", vm.id, placement.node_id),
                serde_json::json!({
                    "vm_id": vm.id.0,
                    "node_id": placement.node_id,
                }),
            )
            .await;
        }
    }

    Ok(())
}

/// node-watch 循环：检测节点下线
async fn node_watch_loop(state: AppState) {
    let mut ticker = tokio::time::interval(Duration::from_secs(15));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        if let Err(e) = node_watch_once(&state).await {
            tracing::warn!(error = %e, "node watch error");
        }
    }
}

/// 单次 node-watch：把心跳超时的节点标记为下线
pub async fn node_watch_once(state: &AppState) -> anyhow::Result<()> {
    let now = gita_vm_core::now_ms();
    let timeout_ms = 30_000i64;

    let nodes = state.repo.list_nodes(None).await?;
    for node in nodes {
        if matches!(node.status.phase, gita_vm_api::node::NodePhase::Online | gita_vm_api::node::NodePhase::Joining)
            && now - node.status.last_heartbeat_ms > timeout_ms
        {
            let mut n = node.clone();
            n.status.phase = gita_vm_api::node::NodePhase::Offline;
            if let Err(e) = state.repo.update_node(&n).await {
                tracing::warn!(node = %n.id, error = %e, "mark node offline failed");
                continue;
            }
            state.emit(
                "vm.node_offline",
                Some(n.id.0.to_string()),
                format!("节点 {} 心跳超时，标记为下线", n.name),
                serde_json::json!({ "node_id": n.id.0, "last_heartbeat_ms": n.status.last_heartbeat_ms }),
            )
            .await;
        }
    }
    Ok(())
}

/// vm-gc 循环：回收过期 VM
async fn vm_gc_loop(state: AppState) {
    let mut ticker = tokio::time::interval(Duration::from_secs(60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        if let Err(e) = vm_gc_once(&state).await {
            tracing::warn!(error = %e, "vm gc error");
        }
    }
}

/// 单次 vm-gc：回收过期的 VM
pub async fn vm_gc_once(state: &AppState) -> anyhow::Result<()> {
    let now = gita_vm_core::now_ms();

    let vms = state.repo.list_vms(None).await?;
    for vm in vms {
        if vm.is_expired(now) {
            tracing::info!(vm = %vm.id, "reclaiming expired vm");
            // 释放 IP 并删除记录（节点侧资源由节点 GC 或 DeleteVm 操作清理）
            let _ = state.repo.release_ip_lease_for_vm(vm.id).await;
            let _ = state.repo.delete_vm(vm.id).await;
            state.emit(
                "vm.expired",
                Some(vm.id.0.to_string()),
                format!("VM {} 已过期，自动回收", vm.name),
                serde_json::json!({ "vm_id": vm.id.0, "name": vm.name }),
            )
            .await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! The loops that keep the cluster in shape.
    //!
    //! Each loop is one decision repeated on a timer: a VM that is
    //! waiting is placed on a node, a node that stopped answering is
    //! marked down, a VM that outlived its welcome is reclaimed. The
    //! decisions are what matter, so each is exercised on its own.

    use super::*;
    use gita_vm_api::ids::{NodeId, VmId};
    use gita_vm_api::node::{
        ConditionStatus, Node, NodeCapabilities, NodeCondition, NodeConditionType, NodePhase,
        NodeSpec, NodeStatus, VmmKind,
    };
    use gita_vm_api::vm::{ImageRef, Vm, VmSpec, VmStatus};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// The cluster, in memory.
    ///
    /// The store keeps the VMs and the nodes and remembers what the
    /// loops did to them.
    #[derive(Default)]
    struct MemoryStore {
        vms: Mutex<Vec<Vm>>,
        nodes: Mutex<Vec<Node>>,
        /// The IPs that were released.
        released: Mutex<Vec<VmId>>,
        /// The events that were emitted.
        events: Mutex<Vec<String>>,
    }

    impl MemoryStore {
        fn vm(&self, id: i64) -> Option<Vm> {
            self.vms.lock().unwrap().iter().find(|v| v.id.0 == id).cloned()
        }

        fn node(&self, id: i64) -> Option<Node> {
            self.nodes.lock().unwrap().iter().find(|n| n.id.0 == id).cloned()
        }
    }

    #[async_trait::async_trait]
    impl gita_vm_core::Repository for MemoryStore {
        async fn list_vms(&self, _owner: Option<gita_vm_api::ids::OwnerRef>) -> gita_vm_core::Result<Vec<Vm>> {
            Ok(self.vms.lock().unwrap().clone())
        }
        async fn list_nodes(&self, _owner: Option<gita_vm_api::ids::OwnerRef>) -> gita_vm_core::Result<Vec<Node>> {
            Ok(self.nodes.lock().unwrap().clone())
        }
        async fn update_vm(&self, vm: &Vm) -> gita_vm_core::Result<()> {
            let mut vms = self.vms.lock().unwrap();
            match vms.iter_mut().find(|v| v.id == vm.id) {
                Some(slot) => {
                    *slot = vm.clone();
                    Ok(())
                }
                None => Err(gita_vm_core::Error::NotFound(format!("vm {} not found", vm.id))),
            }
        }
        async fn update_node(&self, node: &Node) -> gita_vm_core::Result<()> {
            let mut nodes = self.nodes.lock().unwrap();
            match nodes.iter_mut().find(|n| n.id == node.id) {
                Some(slot) => {
                    *slot = node.clone();
                    Ok(())
                }
                None => Err(gita_vm_core::Error::NotFound(format!("node {} not found", node.id))),
            }
        }
        async fn delete_vm(&self, id: VmId) -> gita_vm_core::Result<()> {
            self.vms.lock().unwrap().retain(|v| v.id != id);
            Ok(())
        }
        async fn release_ip_lease_for_vm(&self, vm_id: VmId) -> gita_vm_core::Result<()> {
            self.released.lock().unwrap().push(vm_id);
            Ok(())
        }
        async fn append_event(&self, event: &gita_vm_api::Event) -> gita_vm_core::Result<()> {
            self.events.lock().unwrap().push(event.kind.clone());
            Ok(())
        }
    }

    /// A state whose cluster lives in memory.
    fn state(store: std::sync::Arc<MemoryStore>) -> AppState {
        // The pool is lazy: nothing in these tests touches the
        // database, and the control plane's auth only reads it when a
        // request comes in.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://gita:gita@127.0.0.1:1/gita")
            .unwrap();
        let db = gita_db::Db::from_pool(pool);
        let auth = crate::auth::VmAuth::new(
            db,
            std::sync::Arc::new(gita_api::auth::Auth::new("test-secret".to_string())),
            Some("node-key".to_string()),
        );
        AppState::new(store, crate::state::ControlConfig::default(), auth)
    }

    fn spec() -> VmSpec {
        VmSpec {
            cpu_millis: 1000,
            memory_bytes: 1_000_000_000,
            disk_bytes: 10_000_000_000,
            image: ImageRef {
                name: "ubuntu-24.04".into(),
                digest: None,
            },
            backend: VmmKind::Qemu,
            ..Default::default()
        }
    }

    fn vm(id: i64, status: VmStatus, expires_at_ms: Option<i64>) -> Vm {
        Vm {
            id: VmId(id),
            name: format!("vm-{id}"),
            owner: gita_vm_api::ids::OwnerRef::system(),
            cell: None,
            node: None,
            spec: spec(),
            status,
            generation: 1,
            observed_generation: 1,
            conditions: vec![],
            rootfs_path: None,
            ip_address: None,
            console_url: None,
            created_at_ms: 0,
            message: None,
            expires_at_ms,
        }
    }

    /// A node that is online, healthy and has room for one VM.
    fn node(id: i64, last_heartbeat_ms: i64) -> Node {
        Node {
            id: NodeId(id),
            name: format!("worker-{id}"),
            cell: None,
            spec: NodeSpec {
                cpu_millis: 8000,
                memory_bytes: 16_000_000_000,
                disk_bytes: 100_000_000_000,
                capabilities: NodeCapabilities {
                    vmm: vec![VmmKind::Qemu],
                    arch: vec!["x86_64".into()],
                    nested_virt: true,
                    gpu_passthrough: false,
                    max_vcpus: 64,
                    max_memory_bytes: 16_000_000_000,
                },
            },
            status: NodeStatus {
                alloc_cpu_millis: 0,
                alloc_memory_bytes: 0,
                alloc_disk_bytes: 0,
                running_vms: 0,
                phase: NodePhase::Online,
                last_heartbeat_ms,
            },
            taints: vec![],
            labels: BTreeMap::new(),
            owner: gita_vm_api::ids::OwnerRef::system(),
            conditions: vec![NodeCondition {
                kind: NodeConditionType::Ready,
                status: ConditionStatus::True,
                last_transition_ms: 0,
                reason: None,
                message: None,
            }],
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    /// A VM that is waiting for a node is put on one.
    #[tokio::test]
    async fn a_waiting_vm_is_scheduled_onto_a_node() {
        let store = std::sync::Arc::new(MemoryStore::default());
        store.vms.lock().unwrap().push(vm(1, VmStatus::Pending, None));
        store.nodes.lock().unwrap().push(node(7, gita_vm_core::now_ms()));

        reconcile_once(&state(store.clone())).await.unwrap();

        let vm = store.vm(1).unwrap();
        assert_eq!(vm.status, VmStatus::Provisioning);
        assert_eq!(vm.node, Some(NodeId(7)));
        assert!(
            store.events.lock().unwrap().iter().any(|e| e == "vm.scheduled"),
            "the placement must be announced: {:?}",
            store.events.lock().unwrap()
        );
    }

    /// A VM that is already running is left alone: the reconcile is
    /// about the ones that wait, not about the ones that run.
    #[tokio::test]
    async fn a_running_vm_is_left_alone() {
        let store = std::sync::Arc::new(MemoryStore::default());
        store.vms.lock().unwrap().push(vm(1, VmStatus::Running, None));
        store.nodes.lock().unwrap().push(node(7, gita_vm_core::now_ms()));

        reconcile_once(&state(store.clone())).await.unwrap();

        let vm = store.vm(1).unwrap();
        assert_eq!(vm.status, VmStatus::Running);
        assert_eq!(vm.node, None);
    }

    /// A VM that fits nowhere waits. There is no node to put it on,
    /// and a VM on no node is the truth.
    #[tokio::test]
    async fn a_vm_with_no_node_to_go_to_waits() {
        let store = std::sync::Arc::new(MemoryStore::default());
        store.vms.lock().unwrap().push(vm(1, VmStatus::Pending, None));

        reconcile_once(&state(store.clone())).await.unwrap();

        let vm = store.vm(1).unwrap();
        assert_eq!(vm.status, VmStatus::Pending);
        assert_eq!(vm.node, None);
    }

    /// A node that stopped answering is marked down.
    ///
    /// Its heartbeat is older than the timeout, so the control plane
    /// stops counting on it.
    #[tokio::test]
    async fn a_node_that_stopped_answering_is_marked_down() {
        let store = std::sync::Arc::new(MemoryStore::default());
        let stale = gita_vm_core::now_ms() - 60_000;
        store.nodes.lock().unwrap().push(node(7, stale));

        node_watch_once(&state(store.clone())).await.unwrap();

        let node = store.node(7).unwrap();
        assert_eq!(node.status.phase, NodePhase::Offline);
        assert!(
            store.events.lock().unwrap().iter().any(|e| e == "vm.node_offline"),
            "the node going down must be announced"
        );
    }

    /// A node that is still answering is left online.
    #[tokio::test]
    async fn a_node_that_is_still_answering_stays_online() {
        let store = std::sync::Arc::new(MemoryStore::default());
        store.nodes.lock().unwrap().push(node(7, gita_vm_core::now_ms()));

        node_watch_once(&state(store.clone())).await.unwrap();

        let node = store.node(7).unwrap();
        assert_eq!(node.status.phase, NodePhase::Online);
    }

    /// A node that is already down is not announced again.
    #[tokio::test]
    async fn a_node_that_is_already_down_is_not_announced_again() {
        let store = std::sync::Arc::new(MemoryStore::default());
        let mut n = node(7, gita_vm_core::now_ms() - 60_000);
        n.status.phase = NodePhase::Offline;
        store.nodes.lock().unwrap().push(n);

        node_watch_once(&state(store.clone())).await.unwrap();

        assert!(
            store.events.lock().unwrap().is_empty(),
            "a node that is already down is not news: {:?}",
            store.events.lock().unwrap()
        );
    }

    /// A VM that outlived its welcome is reclaimed: its address is
    /// given back and its record is gone.
    #[tokio::test]
    async fn an_expired_vm_is_reclaimed() {
        let store = std::sync::Arc::new(MemoryStore::default());
        let past = gita_vm_core::now_ms() - 1000;
        store
            .vms
            .lock()
            .unwrap()
            .push(vm(1, VmStatus::Running, Some(past)));

        vm_gc_once(&state(store.clone())).await.unwrap();

        assert!(store.vm(1).is_none(), "the vm must be gone");
        assert_eq!(
            store.released.lock().unwrap().as_slice(),
            [VmId(1)],
            "the address must be given back"
        );
        assert!(
            store.events.lock().unwrap().iter().any(|e| e == "vm.expired"),
            "the reclaim must be announced"
        );
    }

    /// A VM that is still within its time is kept.
    #[tokio::test]
    async fn a_vm_that_is_still_within_its_time_is_kept() {
        let store = std::sync::Arc::new(MemoryStore::default());
        let future = gita_vm_core::now_ms() + 60_000;
        store
            .vms
            .lock()
            .unwrap()
            .push(vm(1, VmStatus::Running, Some(future)));

        vm_gc_once(&state(store.clone())).await.unwrap();

        assert!(store.vm(1).is_some(), "the vm must be kept");
        assert!(store.released.lock().unwrap().is_empty());
    }

    /// A VM that was never given a deadline never expires.
    #[tokio::test]
    async fn a_vm_without_a_deadline_never_expires() {
        let store = std::sync::Arc::new(MemoryStore::default());
        store.vms.lock().unwrap().push(vm(1, VmStatus::Running, None));

        vm_gc_once(&state(store.clone())).await.unwrap();

        assert!(store.vm(1).is_some(), "the vm must be kept");
    }
}
