//! # gita-vm-driver-vmm
//!
//! VMM 抽象层：统一接口，qemu 后端实现。

pub mod error;
pub mod qemu;

pub use error::{Error, Result};
pub use qemu::QemuDriver;

use async_trait::async_trait;
use gita_vm_api::node::VmmKind;
use gita_vm_api::vm::VmSpec;
use std::path::Path;

/// VMM 驱动 trait
#[async_trait]
pub trait VmmDriver: Send + Sync {
    /// 后端类型
    fn kind(&self) -> VmmKind;

    /// 创建 VM 定义（不启动）
    async fn define_vm(&self, vm_id: &str, spec: &VmSpec, work_dir: &Path) -> Result<()>;

    /// 启动 VM
    async fn start_vm(&self, vm_id: &str) -> Result<()>;

    /// 停止 VM
    async fn stop_vm(&self, vm_id: &str, force: bool) -> Result<()>;

    /// 删除 VM（取消定义 + 删除磁盘）
    async fn delete_vm(&self, vm_id: &str) -> Result<()>;

    /// 获取 VM 状态
    async fn get_vm_status(&self, vm_id: &str) -> Result<VmPowerState>;

    /// 获取控制台 URL（VNC/串口）
    async fn get_console(&self, vm_id: &str) -> Result<Option<String>>;

    /// 热插 CPU/内存
    async fn hotplug(
        &self,
        vm_id: &str,
        cpu_millis: Option<u64>,
        memory_bytes: Option<u64>,
    ) -> Result<()>;

    /// 扩容磁盘
    async fn resize_disk(&self, vm_id: &str, new_size_bytes: u64) -> Result<()>;

    /// 创建快照
    async fn create_snapshot(&self, vm_id: &str, snapshot_path: &Path) -> Result<()>;

    /// 恢复快照
    async fn restore_snapshot(&self, vm_id: &str, snapshot_path: &Path) -> Result<()>;
}

/// VM 电源状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmPowerState {
    Undefined,
    Shutoff,
    Running,
    Paused,
    Crashed,
}

impl std::fmt::Display for VmPowerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmPowerState::Undefined => write!(f, "undefined"),
            VmPowerState::Shutoff => write!(f, "shutoff"),
            VmPowerState::Running => write!(f, "running"),
            VmPowerState::Paused => write!(f, "paused"),
            VmPowerState::Crashed => write!(f, "crashed"),
        }
    }
}

/// 根据 VMM 类型创建驱动实例
pub fn create_driver(kind: VmmKind) -> Box<dyn VmmDriver> {
    match kind {
        VmmKind::Qemu => Box::new(QemuDriver::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_driver_qemu() {
        let driver = create_driver(VmmKind::Qemu);
        assert_eq!(driver.kind(), VmmKind::Qemu);
    }

    /// The power state is shown to whoever looks at a VM; the words
    /// are what they read.
    #[test]
    fn the_power_state_is_shown_as_a_word() {
        assert_eq!(VmPowerState::Undefined.to_string(), "undefined");
        assert_eq!(VmPowerState::Shutoff.to_string(), "shutoff");
        assert_eq!(VmPowerState::Running.to_string(), "running");
        assert_eq!(VmPowerState::Paused.to_string(), "paused");
        assert_eq!(VmPowerState::Crashed.to_string(), "crashed");
    }
}
