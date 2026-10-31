//! 虚拟机状态机
//!
//! 定义合法的状态转换，防止非法操作

use gita_vm_api::vm::{Vm, VmStatus};
use crate::Error;

/// 状态机触发事件
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmEvent {
    /// 调度选定节点
    Scheduled,
    /// 资源创建开始
    ProvisionStart,
    /// 资源创建完成，VM 启动中
    Provisioned,
    /// VM 启动成功
    Started,
    /// 停止请求
    StopRequested,
    /// 停止完成
    Stopped,
    /// 重启请求
    RestartRequested,
    /// 删除请求
    DeleteRequested,
    /// 删除完成（资源已回收）
    Deleted,
    /// 迁移开始
    MigrateStart,
    /// 迁移完成
    Migrated,
    /// 发生错误
    Failed(String),
    /// 熔断（反复失败）
    Quarantine,
    /// 从熔断恢复
    Recover,
}

/// 合法转换表：(当前状态, 事件) -> 新状态
pub fn transition(current: VmStatus, event: VmEvent) -> Result<VmStatus, Error> {
    use VmStatus::*;
    let event_desc = format!("{event:?}");

    match (current, event) {
        // Pending -> Provisioning
        (Pending, VmEvent::Scheduled) => Ok(Provisioning),
        // Provisioning -> Running / Failed
        (Provisioning, VmEvent::Provisioned) => Ok(Running),
        (Provisioning, VmEvent::Failed(_)) => Ok(Failed),
        // Running -> Stopping / Migrating / Failed / Quarantined
        (Running, VmEvent::StopRequested) => Ok(Stopping),
        (Running, VmEvent::MigrateStart) => Ok(Migrating),
        (Running, VmEvent::Failed(_)) => Ok(Failed),
        (Running, VmEvent::Quarantine) => Ok(Quarantined),
        // Stopping -> Stopped / Failed
        (Stopping, VmEvent::Stopped) => Ok(Stopped),
        (Stopping, VmEvent::Failed(_)) => Ok(Failed),
        // Stopped -> Provisioning (重启) / Deleted / Failed
        (Stopped, VmEvent::RestartRequested) => Ok(Provisioning),
        (Stopped, VmEvent::DeleteRequested) => Ok(Deleted),
        (Stopped, VmEvent::Failed(_)) => Ok(Failed),
        // Migrating -> Running / Failed
        (Migrating, VmEvent::Migrated) => Ok(Running),
        (Migrating, VmEvent::Failed(_)) => Ok(Failed),
        // Failed -> Provisioning (重试) / Quarantined / Deleted
        (Failed, VmEvent::RestartRequested) => Ok(Provisioning),
        (Failed, VmEvent::Quarantine) => Ok(Quarantined),
        (Failed, VmEvent::DeleteRequested) => Ok(Deleted),
        // Quarantined -> Provisioning (人工恢复) / Deleted
        (Quarantined, VmEvent::Recover) => Ok(Provisioning),
        (Quarantined, VmEvent::DeleteRequested) => Ok(Deleted),
        // 已经是终态，忽略重复事件
        (Failed | Quarantined, VmEvent::Failed(_)) => Ok(current),
        (Deleted, VmEvent::DeleteRequested) => Ok(Deleted),
        // 非法转换
        _ => Err(Error::InvalidTransition(format!(
            "VM 非法状态转换：{current:?} --({event_desc})--> ?"
        ))),
    }
}

/// 应用事件到 VM，返回新的 VM（若状态变更）
pub fn apply_event(vm: &Vm, event: VmEvent) -> Result<Vm, Error> {
    let event_desc = format!("{event:?}");
    let new_status = transition(vm.status, event)?;
    if new_status == vm.status {
        return Ok(vm.clone());
    }
    let mut new_vm = vm.clone();
    new_vm.status = new_status;
    new_vm.observed_generation = new_vm.generation; // 状态变更时同步 generation
    new_vm.conditions.push(gita_vm_api::vm::VmCondition {
        kind: match new_status {
            VmStatus::Running => gita_vm_api::vm::VmConditionType::Ready,
            VmStatus::Provisioning => gita_vm_api::vm::VmConditionType::NetworkReady,
            _ => gita_vm_api::vm::VmConditionType::Ready,
        },
        status: gita_vm_api::node::ConditionStatus::True,
        last_transition_ms: crate::now_ms(),
        reason: Some(event_desc),
    });
    Ok(new_vm)
}

/// 从错误恢复：重置为 Provisioning 重试
pub fn recover_from_failure(vm: &Vm) -> Result<Vm, Error> {
    apply_event(vm, VmEvent::RestartRequested)
}

/// 判断是否可以对该状态发起删除
pub fn can_delete(status: VmStatus) -> bool {
    matches!(status, VmStatus::Stopped | VmStatus::Failed | VmStatus::Quarantined)
}

/// 判断是否可以发起迁移
pub fn can_migrate(status: VmStatus) -> bool {
    matches!(status, VmStatus::Running)
}

/// 判断是否可以发起停止
pub fn can_stop(status: VmStatus) -> bool {
    matches!(status, VmStatus::Running)
}

/// 判断是否可以发起重启
pub fn can_restart(status: VmStatus) -> bool {
    matches!(status, VmStatus::Running | VmStatus::Stopped | VmStatus::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gita_vm_api::vm::{Vm, VmSpec, VmStatus, ImageRef};
    use gita_vm_api::OwnerRef;
    use gita_vm_api::node::VmmKind;
    use std::collections::BTreeMap;

    fn base_vm(status: VmStatus) -> Vm {
        Vm {
            id: gita_vm_api::VmId(1),
            name: "test-vm".into(),
            owner: OwnerRef::system(),
            cell: None,
            node: None,
            spec: VmSpec {
                cpu_millis: 1000,
                memory_bytes: 1_000_000_000,
                disk_bytes: 10_000_000_000,
                image: ImageRef { name: "ubuntu-24.04".into(), digest: None },
                kernel: None,
                networks: vec![],
                backend: VmmKind::Qemu,
                arch: None,
                user_data: None,
                metadata: BTreeMap::new(),
                template: None,
                affinity: None,
                tolerations: vec![],
                priority: 0,
            },
            status,
            generation: 1,
            observed_generation: 1,
            conditions: vec![],
            rootfs_path: None,
            ip_address: None,
            console_url: None,
            created_at_ms: 0,
            message: None,
            expires_at_ms: None,
        }
    }

    #[test]
    fn pending_to_provisioning() {
        let vm = base_vm(VmStatus::Pending);
        let new = apply_event(&vm, VmEvent::Scheduled).unwrap();
        assert_eq!(new.status, VmStatus::Provisioning);
    }

    #[test]
    fn provisioning_to_running() {
        let vm = base_vm(VmStatus::Provisioning);
        let new = apply_event(&vm, VmEvent::Provisioned).unwrap();
        assert_eq!(new.status, VmStatus::Running);
    }

    #[test]
    fn provisioning_to_failed() {
        let vm = base_vm(VmStatus::Provisioning);
        let new = apply_event(&vm, VmEvent::Failed("create failed".into())).unwrap();
        assert_eq!(new.status, VmStatus::Failed);
    }

    #[test]
    fn running_to_stopping_to_stopped() {
        let vm = base_vm(VmStatus::Running);
        let new = apply_event(&vm, VmEvent::StopRequested).unwrap();
        assert_eq!(new.status, VmStatus::Stopping);
        let new = apply_event(&new, VmEvent::Stopped).unwrap();
        assert_eq!(new.status, VmStatus::Stopped);
    }

    #[test]
    fn running_to_migrating_to_running() {
        let vm = base_vm(VmStatus::Running);
        let new = apply_event(&vm, VmEvent::MigrateStart).unwrap();
        assert_eq!(new.status, VmStatus::Migrating);
        let new = apply_event(&new, VmEvent::Migrated).unwrap();
        assert_eq!(new.status, VmStatus::Running);
    }

    #[test]
    fn failed_can_retry() {
        let vm = base_vm(VmStatus::Failed);
        let new = apply_event(&vm, VmEvent::RestartRequested).unwrap();
        assert_eq!(new.status, VmStatus::Provisioning);
    }

    #[test]
    fn failed_can_quarantine() {
        let vm = base_vm(VmStatus::Failed);
        let new = apply_event(&vm, VmEvent::Quarantine).unwrap();
        assert_eq!(new.status, VmStatus::Quarantined);
    }

    #[test]
    fn quarantined_can_recover() {
        let vm = base_vm(VmStatus::Quarantined);
        let new = apply_event(&vm, VmEvent::Recover).unwrap();
        assert_eq!(new.status, VmStatus::Provisioning);
    }

    #[test]
    fn invalid_transition_rejected() {
        let vm = base_vm(VmStatus::Running);
        // Running 不能直接跳到 Stopped
        assert!(apply_event(&vm, VmEvent::Stopped).is_err());
    }

    #[test]
    fn terminal_states_idempotent() {
        let vm = base_vm(VmStatus::Failed);
        // 重复 Failed 事件不改变状态
        let new = apply_event(&vm, VmEvent::Failed("again".into())).unwrap();
        assert_eq!(new.status, VmStatus::Failed);
        assert_eq!(new, vm); // 完全相同（因为状态未变）
    }

    #[test]
    fn can_delete_checks() {
        assert!(can_delete(VmStatus::Stopped));
        assert!(can_delete(VmStatus::Failed));
        assert!(can_delete(VmStatus::Quarantined));
        assert!(!can_delete(VmStatus::Running));
        assert!(!can_delete(VmStatus::Provisioning));
        assert!(!can_delete(VmStatus::Pending));
    }

    #[test]
    fn can_migrate_checks() {
        assert!(can_migrate(VmStatus::Running));
        assert!(!can_migrate(VmStatus::Stopped));
        assert!(!can_migrate(VmStatus::Failed));
    }

    #[test]
    fn can_stop_checks() {
        assert!(can_stop(VmStatus::Running));
        assert!(!can_stop(VmStatus::Stopped));
    }

    #[test]
    fn can_restart_checks() {
        assert!(can_restart(VmStatus::Running));
        assert!(can_restart(VmStatus::Stopped));
        assert!(can_restart(VmStatus::Failed));
        assert!(!can_restart(VmStatus::Pending));
        assert!(!can_restart(VmStatus::Provisioning));
    }
}