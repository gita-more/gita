//! Cloud Hypervisor 驱动实现（存根）

use crate::{Error, Result, VmPowerState, VmmDriver};
use async_trait::async_trait;
use gita_vm_api::node::VmmKind;
use gita_vm_api::vm::VmSpec;
use std::path::Path;

pub struct CloudHypervisorDriver;

impl CloudHypervisorDriver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl VmmDriver for CloudHypervisorDriver {
    fn kind(&self) -> VmmKind {
        VmmKind::CloudHypervisor
    }

    async fn define_vm(&self, _vm_id: &str, _spec: &VmSpec, _work_dir: &Path) -> Result<()> {
        Err(Error::OperationFailed("Cloud Hypervisor driver not implemented".into()))
    }

    async fn start_vm(&self, _vm_id: &str) -> Result<()> {
        Err(Error::OperationFailed("Cloud Hypervisor driver not implemented".into()))
    }

    async fn stop_vm(&self, _vm_id: &str, _force: bool) -> Result<()> {
        Err(Error::OperationFailed("Cloud Hypervisor driver not implemented".into()))
    }

    async fn delete_vm(&self, _vm_id: &str) -> Result<()> {
        Err(Error::OperationFailed("Cloud Hypervisor driver not implemented".into()))
    }

    async fn get_vm_status(&self, _vm_id: &str) -> Result<VmPowerState> {
        Err(Error::OperationFailed("Cloud Hypervisor driver not implemented".into()))
    }

    async fn get_console(&self, _vm_id: &str) -> Result<Option<String>> {
        Err(Error::OperationFailed("Cloud Hypervisor driver not implemented".into()))
    }

    async fn hotplug(&self, _vm_id: &str, _cpu_millis: Option<u64>, _memory_bytes: Option<u64>) -> Result<()> {
        Err(Error::OperationFailed("Cloud Hypervisor driver not implemented".into()))
    }

    async fn resize_disk(&self, _vm_id: &str, _new_size_bytes: u64) -> Result<()> {
        Err(Error::OperationFailed("Cloud Hypervisor driver not implemented".into()))
    }

    async fn create_snapshot(&self, _vm_id: &str, _snapshot_path: &Path) -> Result<()> {
        Err(Error::OperationFailed("Cloud Hypervisor driver not implemented".into()))
    }

    async fn restore_snapshot(&self, _vm_id: &str, _snapshot_path: &Path) -> Result<()> {
        Err(Error::OperationFailed("Cloud Hypervisor driver not implemented".into()))
    }
}

impl Default for CloudHypervisorDriver {
    fn default() -> Self {
        Self::new()
    }
}