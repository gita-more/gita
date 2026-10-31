//! 操作引擎：有序、幂等的步骤执行
//!
//! 核心职责：
//! 1. 为每个操作构建步骤序列
//! 2. 按顺序执行步骤，支持重试、幂等、补偿
//! 3. 状态持久化（每步完成后写回 DB）
//! 4. 失败时执行补偿步骤回滚

use gita_vm_api::ids::{OperationId, VmId};
use gita_vm_api::operation::{Operation, OperationKind, OperationStatus, OperationStep, StepKind, StepStatus};
use crate::repository::Repository;
use crate::{Error, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 操作执行器 trait：具体的步骤实现
#[async_trait]
pub trait StepExecutor: Send + Sync {
    /// 执行单个步骤
    async fn execute(&self, step: &mut OperationStep, ctx: &mut ExecutionContext) -> Result<()>;

    /// 补偿/回滚步骤（失败时调用）
    async fn compensate(&self, step: &OperationStep, ctx: &mut ExecutionContext) -> Result<()>;
}

/// 执行上下文：步骤间共享状态
#[derive(Debug, Default)]
pub struct ExecutionContext {
    /// 步骤输出缓存（按 idempotency_key）
    pub outputs: HashMap<String, serde_json::Value>,
    /// 当前 VM 状态（可选）
    pub vm: Option<gita_vm_api::Vm>,
    /// 当前节点信息
    pub node: Option<gita_vm_api::Node>,
}

/// 操作引擎
pub struct OperationEngine {
    repo: Arc<dyn Repository>,
    executors: Arc<Mutex<HashMap<OperationKind, std::sync::Arc<dyn StepExecutor>>>>,
}

impl OperationEngine {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self {
            repo,
            executors: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 注册某种操作类型的执行器
    pub async fn register_executor(&self, kind: OperationKind, executor: std::sync::Arc<dyn StepExecutor>) {
        let mut map = self.executors.lock().await;
        map.insert(kind, executor);
    }

    /// 创建并执行一个完整操作
    pub async fn execute_operation(&self, mut op: Operation) -> Result<Operation> {
        // 1. 持久化初始状态
        self.repo.create_operation(&op).await?;

        // 2. 标记运行中
        op.mark_running();
        self.repo.update_operation(&op).await?;

        // 3. 执行步骤循环
        loop {
            let step_idx = op.current_step;
            if step_idx >= op.steps.len() {
                // 所有步骤完成
                op.mark_succeeded();
                self.repo.update_operation(&op).await?;
                break;
            }

            let executor: Option<std::sync::Arc<dyn StepExecutor>> = {
                let executors = self.executors.lock().await;
                executors.get(&op.kind).cloned()
            };

            if executor.is_none() {
                let err = format!("未注册操作类型 {:?} 的执行器", op.kind);
                op.steps[step_idx].status = StepStatus::Failed;
                op.steps[step_idx].error = Some(err.clone());
                op.mark_failed(err);
                self.repo.update_operation(&op).await?;
                break;
            }

            let mut ctx = ExecutionContext::default();

            // 执行当前步骤（带重试）。为了避免在借用 op.steps 的同时持久化 op，
            // 先把步骤信息读出来，循环里只通过索引写入。
            let (max_retries, mut attempt) = {
                let step = &op.steps[step_idx];
                (step.retries, step.attempt)
            };

            loop {
                {
                    let step = &mut op.steps[step_idx];
                    step.status = StepStatus::Running;
                    step.started_at_ms = Some(crate::now_ms());
                    step.attempt = attempt;
                }
                self.repo.update_operation(&op).await?;

                let result = {
                    let step = &mut op.steps[step_idx];
                    executor.as_ref().unwrap().execute(step, &mut ctx).await
                };
                match result {
                    Ok(()) => {
                        let step = &mut op.steps[step_idx];
                        step.status = StepStatus::Succeeded;
                        step.finished_at_ms = Some(crate::now_ms());
                        // 缓存幂等输出
                        if let Some(ref key) = step.idempotency_key {
                            ctx.outputs.insert(key.clone(), step.output.clone());
                        }
                        break;
                    }
                    Err(e) => {
                        attempt += 1;
                        {
                            let step = &mut op.steps[step_idx];
                            step.error = Some(e.to_string());
                        }
                        if attempt > max_retries {
                            {
                                let step = &mut op.steps[step_idx];
                                step.status = StepStatus::Failed;
                                step.finished_at_ms = Some(crate::now_ms());
                            }
                            // 执行补偿（回滚已完成步骤）
                            self.compensate_completed(&mut op, &mut ctx).await?;
                            op.mark_failed(e.to_string());
                            self.repo.update_operation(&op).await?;
                            return Ok(op);
                        }
                        // 重试前短暂等待
                        tokio::time::sleep(tokio::time::Duration::from_millis(100 * attempt as u64)).await;
                    }
                }
            }

            // 推进到下一步
            op.advance_step();
            self.repo.update_operation(&op).await?;
        }

        Ok(op)
    }

    /// 执行补偿：反向遍历已成功步骤
    async fn compensate_completed(&self, op: &mut Operation, ctx: &mut ExecutionContext) -> Result<()> {
        let executors = self.executors.lock().await;
        let executor = executors.get(&op.kind);

        // 反向遍历已完成的步骤
        for i in (0..op.current_step).rev() {
            let step = &op.steps[i];
            if step.status == StepStatus::Succeeded {
                if let Some(exec) = executor {
                    let _ = exec.compensate(step, ctx).await;
                }
                // 标记为已补偿（可选）
                op.steps[i].status = StepStatus::Skipped;
            }
        }
        Ok(())
    }

    /// 取消正在执行的操作
    pub async fn cancel_operation(&self, op_id: OperationId) -> Result<()> {
        let mut op = self.repo.get_operation(op_id).await?
            .ok_or_else(|| Error::not_found(format!("Operation {op_id}")))?;
        op.status = OperationStatus::Cancelling;
        self.repo.update_operation(&op).await?;

        // 补偿已完成步骤
        let mut ctx = ExecutionContext::default();
        self.compensate_completed(&mut op, &mut ctx).await?;

        op.status = OperationStatus::Cancelled;
        op.finished_at_ms = Some(crate::now_ms());
        self.repo.update_operation(&op).await?;
        Ok(())
    }
}

/// 内置步骤执行器构建器：为常见操作类型提供标准步骤
pub mod builders {
    use super::*;
    use gita_vm_api::node::Node;
    use gita_vm_api::vm::Vm;

    /// 构建 CreateVm 操作的标准步骤
    pub fn build_create_vm(vm: &Vm, _node: &Node, network_config: &NetworkConfig) -> Operation {
        let spec = &vm.spec;
        let steps = vec![
            // 1. 分配 IP（控制面）
            OperationStep {
                kind: StepKind::Control,
                name: "allocate_ip".into(),
                params: serde_json::json!({
                    "network": network_config.network_name,
                    "vm_id": vm.id.0,
                }),
                idempotency_key: Some(format!("alloc-ip-{}", vm.id.0)),
                retries: 3,
                ..Default::default()
            },
            // 2. 创建磁盘镜像（控制面/节点）
            OperationStep {
                kind: StepKind::Node,
                name: "create_disk".into(),
                params: serde_json::json!({
                    "vm_id": vm.id.0,
                    "disk_bytes": spec.disk_bytes,
                    "base_image": spec.image.name,
                }),
                idempotency_key: Some(format!("create-disk-{}", vm.id.0)),
                retries: 2,
                ..Default::default()
            },
            // 3. 定义 VM（节点：libvirt/QEMU define）
            OperationStep {
                kind: StepKind::Node,
                name: "define_vm".into(),
                params: serde_json::json!({
                    "vm_id": vm.id.0,
                    "name": vm.name,
                    "cpu_millis": spec.cpu_millis,
                    "memory_bytes": spec.memory_bytes,
                    "vmm": spec.backend.as_str(),
                }),
                idempotency_key: Some(format!("define-vm-{}", vm.id.0)),
                retries: 2,
                ..Default::default()
            },
            // 4. 启动 VM（节点）
            OperationStep {
                kind: StepKind::Node,
                name: "start_vm".into(),
                params: serde_json::json!({
                    "vm_id": vm.id.0,
                }),
                idempotency_key: Some(format!("start-vm-{}", vm.id.0)),
                retries: 3,
                ..Default::default()
            },
            // 5. 等待 VM Ready（控制面：轮询 agent 心跳）
            OperationStep {
                kind: StepKind::Wait,
                name: "wait_ready".into(),
                params: serde_json::json!({
                    "vm_id": vm.id.0,
                    "timeout_secs": 120,
                }),
                timeout_ms: Some(120_000),
                retries: 1,
                ..Default::default()
            },
            // 6. 注入 userdata/metadata（节点：cloud-init）
            OperationStep {
                kind: StepKind::Node,
                name: "inject_userdata".into(),
                params: serde_json::json!({
                    "vm_id": vm.id.0,
                    "user_data": spec.user_data,
                    "metadata": spec.metadata,
                }),
                idempotency_key: Some(format!("inject-ud-{}", vm.id.0)),
                retries: 2,
                ..Default::default()
            },
        ];

        Operation::new(OperationKind::CreateVm, vm.id, steps)
    }

    /// 构建 StartVm 操作
    pub fn build_start_vm(vm_id: VmId) -> Operation {
        let steps = vec![
            OperationStep {
                kind: StepKind::Node,
                name: "start_vm".into(),
                params: serde_json::json!({ "vm_id": vm_id.0 }),
                idempotency_key: Some(format!("start-vm-{}", vm_id.0)),
                retries: 3,
                ..Default::default()
            },
            OperationStep {
                kind: StepKind::Wait,
                name: "wait_running".into(),
                params: serde_json::json!({ "vm_id": vm_id.0, "timeout_secs": 60 }),
                timeout_ms: Some(60_000),
                ..Default::default()
            },
        ];
        Operation::new(OperationKind::StartVm, vm_id, steps)
    }

    /// 构建 StopVm 操作
    pub fn build_stop_vm(vm_id: VmId, force: bool) -> Operation {
        let steps = vec![
            OperationStep {
                kind: StepKind::Node,
                name: "stop_vm".into(),
                params: serde_json::json!({ "vm_id": vm_id.0, "force": force }),
                idempotency_key: Some(format!("stop-vm-{}", vm_id.0)),
                retries: 3,
                ..Default::default()
            },
            OperationStep {
                kind: StepKind::Wait,
                name: "wait_stopped".into(),
                params: serde_json::json!({ "vm_id": vm_id.0, "timeout_secs": 60 }),
                timeout_ms: Some(60_000),
                ..Default::default()
            },
        ];
        Operation::new(OperationKind::StopVm, vm_id, steps)
    }

    /// 构建 DeleteVm 操作（含补偿：只在失败时回滚）
    pub fn build_delete_vm(vm: &Vm, _node: &Node) -> Operation {
        let steps = vec![
            OperationStep {
                kind: StepKind::Node,
                name: "stop_vm".into(),
                params: serde_json::json!({ "vm_id": vm.id.0, "force": true }),
                idempotency_key: Some(format!("stop-vm-del-{}", vm.id.0)),
                retries: 3,
                ..Default::default()
            },
            OperationStep {
                kind: StepKind::Node,
                name: "undefine_vm".into(),
                params: serde_json::json!({ "vm_id": vm.id.0 }),
                idempotency_key: Some(format!("undefine-vm-{}", vm.id.0)),
                retries: 2,
                ..Default::default()
            },
            OperationStep {
                kind: StepKind::Node,
                name: "delete_disk".into(),
                params: serde_json::json!({ "vm_id": vm.id.0 }),
                idempotency_key: Some(format!("delete-disk-{}", vm.id.0)),
                retries: 2,
                ..Default::default()
            },
            OperationStep {
                kind: StepKind::Control,
                name: "release_ip".into(),
                params: serde_json::json!({ "vm_id": vm.id.0 }),
                idempotency_key: Some(format!("release-ip-{}", vm.id.0)),
                retries: 3,
                ..Default::default()
            },
        ];
        Operation::new(OperationKind::DeleteVm, vm.id, steps)
    }

    /// 网络配置（运行时传入）
    #[derive(Debug, Clone)]
    pub struct NetworkConfig {
        pub network_name: String,
        pub bridge: String,
        pub gateway: String,
        pub netmask: String,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gita_vm_api::operation::{Operation, OperationKind, OperationStep, StepKind, StepStatus};

    #[test]
    fn operation_builder_create_vm() {
        let op = builders::build_create_vm(
            &gita_vm_api::Vm {
                id: gita_vm_api::VmId(1),
                name: "test".into(),
                owner: gita_vm_api::OwnerRef::system(),
                cell: None,
                node: None,
                spec: gita_vm_api::VmSpec {
                    cpu_millis: 2000,
                    memory_bytes: 2_000_000_000,
                    disk_bytes: 10_000_000_000,
                    image: gita_vm_api::vm::ImageRef { name: "ubuntu-24.04".into(), digest: None },
                    kernel: None,
                    networks: vec![],
                    backend: gita_vm_api::node::VmmKind::Qemu,
                    arch: None,
                    user_data: None,
                    metadata: std::collections::BTreeMap::new(),
                    template: None,
                    affinity: None,
                    tolerations: vec![],
                    priority: 0,
                },
                status: gita_vm_api::vm::VmStatus::Pending,
                generation: 1,
                observed_generation: 1,
                conditions: vec![],
                rootfs_path: None,
                ip_address: None,
                console_url: None,
                created_at_ms: 0,
                message: None,
                expires_at_ms: None,
            },
            &gita_vm_api::Node {
                id: gita_vm_api::NodeId(1),
                name: "node1".into(),
                cell: None,
                spec: gita_vm_api::node::NodeSpec {
                    cpu_millis: 8000,
                    memory_bytes: 8_000_000_000,
                    disk_bytes: 100_000_000_000,
                    capabilities: gita_vm_api::node::NodeCapabilities::default(),
                },
                status: gita_vm_api::node::NodeStatus::default(),
                taints: vec![],
                labels: std::collections::BTreeMap::new(),
                owner: gita_vm_api::OwnerRef::system(),
                conditions: vec![],
                created_at_ms: 0,
                updated_at_ms: 0,
            },
            &builders::NetworkConfig {
                network_name: "ci-bridge".into(),
                bridge: "br-ci".into(),
                gateway: "10.42.0.1".into(),
                netmask: "255.255.255.0".into(),
            },
        );

        assert_eq!(op.kind, OperationKind::CreateVm);
        assert_eq!(op.steps.len(), 6);
        assert_eq!(op.steps[0].name, "allocate_ip");
        assert_eq!(op.steps[1].name, "create_disk");
        assert_eq!(op.steps[2].name, "define_vm");
        assert_eq!(op.steps[3].name, "start_vm");
        assert_eq!(op.steps[4].name, "wait_ready");
        assert_eq!(op.steps[5].name, "inject_userdata");
    }

    #[test]
    fn operation_advance_and_complete() {
        let mut op = Operation::new(
            OperationKind::CreateVm,
            gita_vm_api::VmId(1),
            vec![
                OperationStep { kind: StepKind::Control, name: "step1".into(), ..Default::default() },
                OperationStep { kind: StepKind::Node, name: "step2".into(), ..Default::default() },
            ],
        );
        assert_eq!(op.current_step, 0);
        assert!(!op.is_complete());

        op.steps[0].status = StepStatus::Succeeded;
        assert!(op.advance_step()); // -> 1

        op.steps[1].status = StepStatus::Succeeded;
        assert!(!op.advance_step()); // -> 2 (end)

        assert!(op.is_complete());
    }
}