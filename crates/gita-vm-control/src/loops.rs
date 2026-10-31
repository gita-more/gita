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
async fn reconcile_once(state: &AppState) -> anyhow::Result<()> {
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
        let now = gita_vm_core::now_ms();
        let timeout_ms = 30_000i64;

        match state.repo.list_nodes(None).await {
            Ok(nodes) => {
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
            }
            Err(e) => tracing::warn!(error = %e, "node watch: list nodes failed"),
        }
    }
}

/// vm-gc 循环：回收过期 VM
async fn vm_gc_loop(state: AppState) {
    let mut ticker = tokio::time::interval(Duration::from_secs(60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        let now = gita_vm_core::now_ms();

        match state.repo.list_vms(None).await {
            Ok(vms) => {
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
            }
            Err(e) => tracing::warn!(error = %e, "vm gc: list vms failed"),
        }
    }
}
