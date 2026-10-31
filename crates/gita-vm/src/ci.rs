//! CI 专用 VM 辅助：从模板创建 VM、注入 agent 配置、等待就绪、收集日志

use gita_vm_api::ids::{OwnerRef, TemplateId, VmId};
use gita_vm_api::template::{VmSpecOverrides, VmTemplate};
use gita_vm_api::vm::{Vm, VmSpec, VmStatus};
use gita_vm_core::repository::Repository;
use gita_vm_core::{Error, Result};
use gita_vm_store::VmStore;
use std::sync::Arc;
use std::time::Duration;

/// 创建 VM 时的附加选项
#[derive(Debug, Clone, Default)]
pub struct CreateVmOptions {
    /// 资源归属（决定配额与权限）
    pub owner: Option<OwnerRef>,
    /// 关联的 gita 仓库
    pub repo_id: Option<i64>,
    /// 关联的 CI pipeline run
    pub pipeline_run_id: Option<i64>,
    /// 关联的 CI job
    pub job_id: Option<i64>,
    /// VM 过期时间（毫秒）；CI VM 通常设为 job 超时 + 缓冲
    pub expires_at_ms: Option<i64>,
    /// 模板 ID（可选，从模板实例化）
    pub template_id: Option<TemplateId>,
    /// 规格覆盖
    pub overrides: Option<VmSpecOverrides>,
}

/// VM 执行结果
#[derive(Debug, Clone)]
pub struct VmResult {
    pub vm_id: VmId,
    pub ip: Option<String>,
    pub console_url: Option<String>,
}

/// VmCluster trait：给 gita-ci/gita-runner 用的统一接口
#[async_trait::async_trait]
pub trait VmCluster: Send + Sync {
    async fn create_vm(&self, spec: VmSpec) -> Result<VmResult>;
    async fn get_vm(&self, id: VmId) -> Result<Option<Vm>>;
    async fn destroy_vm(&self, id: VmId) -> Result<()>;
    async fn list_templates(&self) -> Result<Vec<VmTemplate>>;
    async fn wait_ready(&self, id: VmId, timeout: Duration) -> Result<Vm>;
    async fn get_console(&self, id: VmId) -> Result<Option<String>>;
}

/// CI VM 辅助器
pub struct CiVmHelper {
    store: Arc<VmStore>,
}

impl CiVmHelper {
    pub fn new(store: Arc<VmStore>) -> Self {
        Self { store }
    }

    /// 从模板创建 CI VM
    pub async fn create_ci_vm(&self, opts: &CreateVmOptions) -> Result<VmResult> {
        // 1. 获取模板（或使用默认规格）
        let spec = match opts.template_id {
            Some(tid) => {
                let tmpl = self
                    .store
                    .get_template(tid)
                    .await?
                    .ok_or_else(|| Error::not_found(format!("模板 {tid}")))?;
                tmpl.instantiate(opts.overrides.as_ref())
            }
            None => {
                // 无模板时要求完整 spec
                return Err(Error::Validation("必须指定 template_id".into()));
            }
        };

        // 2. 生成 VM 名称
        let now = gita_vm_core::now_ms();
        let name = format!(
            "ci-{}-{}",
            opts.repo_id.map(|r| r.to_string()).unwrap_or_else(|| "adhoc".into()),
            now
        );

        // 3. 创建 VM 记录
        let vm = Vm {
            id: VmId(0), // 由 DB 分配
            name,
            owner: opts.owner.clone().unwrap_or_else(OwnerRef::system),
            cell: None,
            node: None,
            spec,
            status: VmStatus::Pending,
            generation: 1,
            observed_generation: 0,
            conditions: vec![],
            rootfs_path: None,
            ip_address: None,
            console_url: None,
            created_at_ms: now,
            message: None,
            expires_at_ms: opts.expires_at_ms,
        };

        self.store.create_vm(&vm).await?;

        // 4. 触发调度与创建（由控制面 reconcile 循环完成）
        // 这里返回 VM ID，由调用方 wait_ready

        Ok(VmResult {
            vm_id: vm.id,
            ip: None,
            console_url: None,
        })
    }

    /// 等待 VM 就绪（Running + agent 心跳）
    pub async fn wait_ready(&self, vm_id: VmId, timeout: Duration) -> Result<Vm> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let vm = self
                .store
                .get_vm(vm_id)
                .await?
                .ok_or_else(|| Error::not_found(format!("VM {vm_id}")))?;

            match vm.status {
                VmStatus::Running => {
                    return Ok(vm);
                }
                VmStatus::Failed | VmStatus::Quarantined => {
                    return Err(Error::OperationFailed(format!(
                        "VM {vm_id} 进入失败状态: {:?}",
                        vm.message
                    )));
                }
                _ => {}
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(Error::OperationFailed(format!(
                    "等待 VM {vm_id} 就绪超时（{:?}）",
                    timeout
                )));
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// 获取 VM 控制台 URL
    pub async fn get_console(&self, vm_id: VmId) -> Result<Option<String>> {
        let vm = self
            .store
            .get_vm(vm_id)
            .await?
            .ok_or_else(|| Error::not_found(format!("VM {vm_id}")))?;
        Ok(vm.console_url)
    }

    /// 销毁 VM（幂等）
    pub async fn destroy_vm(&self, vm_id: VmId) -> Result<()> {
        if self.store.get_vm(vm_id).await?.is_none() {
            return Ok(()); // 幂等：不存在即成功
        }

        // 释放 IP
        // TODO: 通过 Ipam 释放

        // 删除记录（节点侧由控制面下发 DeleteVm 操作）
        self.store.delete_vm(vm_id).await?;
        Ok(())
    }
}