//! CI 专用 VM 辅助：从模板创建 VM、注入 agent 配置、等待就绪、收集日志

use gita_vm_api::ids::{OwnerRef, TemplateId, VmId};
use gita_vm_api::template::{VmSpecOverrides, VmTemplate};
use gita_vm_api::vm::{Vm, VmLinkage, VmSpec, VmStatus};
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

        // The VM is linked to the run it belongs to, so that the
        // control plane knows who is to reclaim it.
        let linkage = VmLinkage {
            repo_id: opts.repo_id,
            pipeline_run_id: opts.pipeline_run_id,
            job_id: opts.job_id,
        };
        // `create_vm` does not report the id it was given, and the
        // caller needs it: it waits for the VM and reclaims it.
        let vm_id = self.store.create_linked_vm(&vm, linkage).await?;

        // 4. 触发调度与创建（由控制面 reconcile 循环完成）
        // 这里返回 VM ID，由调用方 wait_ready

        Ok(VmResult {
            vm_id,
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
#[cfg(test)]
mod tests {
    use super::*;
    use gita_vm_api::template::VmTemplate;

    fn url() -> String {
        std::env::var("GITA_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://gita:gita@127.0.0.1:55432/gita".to_string())
    }

    async fn helper() -> (gita_db::Db, CiVmHelper, Arc<VmStore>) {
        let db = gita_db::new_test_db(&url()).await;
        let store = Arc::new(VmStore::new(db.pool().clone()));
        let helper = CiVmHelper::new(store.clone());
        (db, helper, store)
    }

    fn a_template(name: &str) -> VmTemplate {
        VmTemplate {
            id: TemplateId(0),
            name: name.to_string(),
            version: 1,
            spec: VmSpec {
                cpu_millis: 1000,
                memory_bytes: 1024 * 1024,
                disk_bytes: 2048,
                backend: gita_vm_api::node::VmmKind::Qemu,
                ..Default::default()
            },
            created_at_ms: 1000,
            image_id: None,
            kernel_image_id: None,
            owner: OwnerRef::system(),
            tags: vec![],
            description: String::new(),
        }
    }

    /// A CI VM is created from a template and gets a real id: the
    /// caller needs it to wait for the VM and to reclaim it.
    #[tokio::test]
    async fn a_ci_vm_is_created_from_a_template() {
        let (_db, helper, store) = helper().await;
        store.create_template(&a_template("ubuntu")).await.unwrap();
        let template_id = store.list_templates(None).await.unwrap()[0].id;

        let opts = CreateVmOptions {
            owner: Some(OwnerRef::repo(3)),
            template_id: Some(template_id),
            ..Default::default()
        };
        let result = helper.create_ci_vm(&opts).await.unwrap();
        assert!(result.vm_id.0 > 0, "the vm must get a real id");

        let vm = helper
            .store
            .get_vm(result.vm_id)
            .await
            .unwrap()
            .expect("the vm must be there");
        assert_eq!(vm.owner.owner_type, gita_vm_api::ids::OwnerType::Repo);
        assert_eq!(vm.owner.owner_id, 3);
        assert_eq!(vm.status, VmStatus::Pending);
        assert_eq!(vm.spec.cpu_millis, 1000);
    }

    /// The name of a CI VM says which run it belongs to.
    #[tokio::test]
    async fn a_ci_vm_is_named_after_its_run() {
        let (db, helper, store) = helper().await;
        store.create_template(&a_template("ubuntu")).await.unwrap();
        let template_id = store.list_templates(None).await.unwrap()[0].id;
        // The VM is linked to a repository, so the repository has to
        // exist.
        let user = db.create_user("alice", "alice@example.com", "hash").await.unwrap();
        let repo = db
            .create_repository(user.id, "alice", "widgets", "", false, "main", None)
            .await
            .unwrap();

        let opts = CreateVmOptions {
            repo_id: Some(repo.id),
            template_id: Some(template_id),
            ..Default::default()
        };
        let result = helper.create_ci_vm(&opts).await.unwrap();
        let vm = helper.store.get_vm(result.vm_id).await.unwrap().unwrap();
        assert!(
            vm.name.starts_with(&format!("ci-{}-", repo.id)),
            "the name must name the repository: {}",
            vm.name
        );

        // Without a repository the name says so.
        let opts = CreateVmOptions {
            template_id: Some(template_id),
            ..Default::default()
        };
        let result = helper.create_ci_vm(&opts).await.unwrap();
        let vm = helper.store.get_vm(result.vm_id).await.unwrap().unwrap();
        assert!(
            vm.name.starts_with("ci-adhoc-"),
            "the name must say it is ad hoc: {}",
            vm.name
        );
    }

    /// A CI VM without a template cannot be created: there is no
    /// specification to create it from.
    #[tokio::test]
    async fn a_ci_vm_without_a_template_is_refused() {
        let (_db, helper, _store) = helper().await;
        let err = helper
            .create_ci_vm(&CreateVmOptions::default())
            .await
            .expect_err("the creation must fail");
        assert!(matches!(err, Error::Validation(_)), "got {err:?}");
    }

    /// A template that does not exist is refused.
    #[tokio::test]
    async fn a_ci_vm_from_an_unknown_template_is_refused() {
        let (_db, helper, _store) = helper().await;
        let opts = CreateVmOptions {
            template_id: Some(TemplateId(999)),
            ..Default::default()
        };
        let err = helper
            .create_ci_vm(&opts)
            .await
            .expect_err("the creation must fail");
        assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
    }

    /// The overrides of the caller win over the template's spec.
    #[tokio::test]
    async fn the_overrides_win_over_the_template() {
        let (_db, helper, store) = helper().await;
        store.create_template(&a_template("ubuntu")).await.unwrap();
        let template_id = store.list_templates(None).await.unwrap()[0].id;

        let opts = CreateVmOptions {
            template_id: Some(template_id),
            overrides: Some(VmSpecOverrides {
                cpu_millis: Some(4000),
                memory_bytes: Some(8 * 1024 * 1024),
                disk_bytes: Some(4096),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = helper.create_ci_vm(&opts).await.unwrap();
        let vm = helper.store.get_vm(result.vm_id).await.unwrap().unwrap();
        assert_eq!(vm.spec.cpu_millis, 4000);
        assert_eq!(vm.spec.memory_bytes, 8 * 1024 * 1024);
        assert_eq!(vm.spec.disk_bytes, 4096);
    }

    /// A VM that is running is ready.
    #[tokio::test]
    async fn a_running_vm_is_ready() {
        let (_db, helper, store) = helper().await;
        let vm = Vm {
            id: VmId(0),
            name: "vm-1".to_string(),
            owner: OwnerRef::system(),
            cell: None,
            node: None,
            spec: VmSpec::default(),
            status: VmStatus::Running,
            generation: 1,
            observed_generation: 1,
            conditions: vec![],
            rootfs_path: None,
            ip_address: None,
            console_url: None,
            created_at_ms: 1000,
            message: None,
            expires_at_ms: None,
        };
        store.create_vm(&vm).await.unwrap();
        let id = store.list_vms(None).await.unwrap()[0].id;

        let ready = helper
            .wait_ready(id, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(ready.status, VmStatus::Running);
    }

    /// A VM that failed will never be ready.
    #[tokio::test]
    async fn a_failed_vm_is_never_ready() {
        let (_db, helper, store) = helper().await;
        let mut vm = Vm {
            id: VmId(0),
            name: "vm-1".to_string(),
            owner: OwnerRef::system(),
            cell: None,
            node: None,
            spec: VmSpec::default(),
            status: VmStatus::Failed,
            generation: 1,
            observed_generation: 1,
            conditions: vec![],
            rootfs_path: None,
            ip_address: None,
            console_url: None,
            created_at_ms: 1000,
            message: Some("the image did not boot".to_string()),
            expires_at_ms: None,
        };
        vm.status = VmStatus::Quarantined;
        store.create_vm(&vm).await.unwrap();
        let id = store.list_vms(None).await.unwrap()[0].id;

        let err = helper
            .wait_ready(id, Duration::from_secs(1))
            .await
            .expect_err("the wait must fail");
        assert!(matches!(err, Error::OperationFailed(_)), "got {err:?}");
    }

    /// A VM that never starts is not ready when the wait runs out.
    #[tokio::test]
    async fn waiting_for_a_vm_that_never_starts_times_out() {
        let (_db, helper, store) = helper().await;
        let vm = Vm {
            id: VmId(0),
            name: "vm-1".to_string(),
            owner: OwnerRef::system(),
            cell: None,
            node: None,
            spec: VmSpec::default(),
            status: VmStatus::Pending,
            generation: 1,
            observed_generation: 1,
            conditions: vec![],
            rootfs_path: None,
            ip_address: None,
            console_url: None,
            created_at_ms: 1000,
            message: None,
            expires_at_ms: None,
        };
        store.create_vm(&vm).await.unwrap();
        let id = store.list_vms(None).await.unwrap()[0].id;

        let err = helper
            .wait_ready(id, Duration::from_millis(10))
            .await
            .expect_err("the wait must fail");
        assert!(matches!(err, Error::OperationFailed(_)), "got {err:?}");
    }

    /// Waiting for a VM that does not exist is refused.
    #[tokio::test]
    async fn waiting_for_an_unknown_vm_is_refused() {
        let (_db, helper, _store) = helper().await;
        let err = helper
            .wait_ready(VmId(999), Duration::from_millis(10))
            .await
            .expect_err("the wait must fail");
        assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
    }

    /// Destroying a VM removes it; destroying one that is already gone
    /// succeeds, so that a retry after a failure is harmless.
    #[tokio::test]
    async fn destroying_a_vm_is_idempotent() {
        let (_db, helper, store) = helper().await;
        let vm = Vm {
            id: VmId(0),
            name: "vm-1".to_string(),
            owner: OwnerRef::system(),
            cell: None,
            node: None,
            spec: VmSpec::default(),
            status: VmStatus::Stopped,
            generation: 1,
            observed_generation: 1,
            conditions: vec![],
            rootfs_path: None,
            ip_address: None,
            console_url: None,
            created_at_ms: 1000,
            message: None,
            expires_at_ms: None,
        };
        store.create_vm(&vm).await.unwrap();
        let id = store.list_vms(None).await.unwrap()[0].id;

        helper.destroy_vm(id).await.unwrap();
        assert!(helper.store.get_vm(id).await.unwrap().is_none());

        // Destroying it again succeeds.
        helper.destroy_vm(id).await.unwrap();
    }

    /// The console of a VM is the URL it was given.
    #[tokio::test]
    async fn the_console_of_a_vm_is_its_url() {
        let (_db, helper, store) = helper().await;
        let mut vm = Vm {
            id: VmId(0),
            name: "vm-1".to_string(),
            owner: OwnerRef::system(),
            cell: None,
            node: None,
            spec: VmSpec::default(),
            status: VmStatus::Running,
            generation: 1,
            observed_generation: 1,
            conditions: vec![],
            rootfs_path: None,
            ip_address: None,
            console_url: Some("qmp+unix:///tmp/qmp.sock".to_string()),
            created_at_ms: 1000,
            message: None,
            expires_at_ms: None,
        };
        vm.console_url = Some("qmp+unix:///tmp/qmp.sock".to_string());
        store.create_vm(&vm).await.unwrap();
        let id = store.list_vms(None).await.unwrap()[0].id;

        let console = helper.get_console(id).await.unwrap();
        assert_eq!(console.as_deref(), Some("qmp+unix:///tmp/qmp.sock"));
    }

    /// The console of a VM that does not exist cannot be read.
    #[tokio::test]
    async fn the_console_of_an_unknown_vm_cannot_be_read() {
        let (_db, helper, _store) = helper().await;
        let err = helper
            .get_console(VmId(999))
            .await
            .expect_err("the read must fail");
        assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
    }
}
