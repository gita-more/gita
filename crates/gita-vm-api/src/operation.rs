//! 操作引擎类型：有序、幂等的步骤执行

use crate::ids::{NodeId, OperationId, VmId};
use serde::{Deserialize, Serialize};

/// 操作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    /// 创建 VM
    CreateVm,
    /// 启动 VM
    StartVm,
    /// 停止 VM
    StopVm,
    /// 重启 VM
    RestartVm,
    /// 删除 VM
    DeleteVm,
    /// 迁移 VM
    MigrateVm,
    /// 创建快照
    CreateSnapshot,
    /// 恢复快照
    RestoreSnapshot,
    /// 扩容磁盘
    ResizeDisk,
    /// 热插 CPU/内存
    HotplugResources,
    /// 更新网络
    UpdateNetwork,
    /// 注入用户数据
    InjectUserData,
}

/// 操作状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    /// 等待调度
    Pending,
    /// 正在执行
    Running,
    /// 成功完成
    Succeeded,
    /// 失败
    Failed,
    /// 取消中
    Cancelling,
    /// 已取消
    Cancelled,
}

/// 步骤类型
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// 在控制面执行（如分配 IP、更新 DB）
    #[default]
    Control,
    /// 下发给节点执行（如启动 QEMU）
    Node,
    /// 等待条件满足（如 VM 状态变为 Running）
    Wait,
    /// 补偿/回滚步骤
    Compensate,
}

/// 步骤状态
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    #[default]
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
}

/// 单个操作步骤
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OperationStep {
    pub kind: StepKind,
    /// 步骤名（用于日志/调试）
    pub name: String,
    /// 执行参数（JSON）
    pub params: serde_json::Value,
    /// 幂等键：相同幂等键的步骤只执行一次
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// 超时毫秒
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// 重试次数
    #[serde(default)]
    pub retries: u32,
    /// 当前重试次数
    #[serde(default)]
    pub attempt: u32,
    /// 步骤状态
    #[serde(default = "default_step_status")]
    pub status: StepStatus,
    /// 步骤输出（JSON）
    #[serde(default)]
    pub output: serde_json::Value,
    /// 错误信息
    #[serde(default)]
    pub error: Option<String>,
    /// 开始时间（毫秒）
    #[serde(default)]
    pub started_at_ms: Option<i64>,
    /// 结束时间（毫秒）
    #[serde(default)]
    pub finished_at_ms: Option<i64>,
}

fn default_step_status() -> StepStatus {
    StepStatus::Pending
}

/// 一个操作：由一系列有序步骤组成，支持幂等、重试、补偿
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Operation {
    pub id: OperationId,
    pub kind: OperationKind,
    /// 目标 VM
    pub vm_id: VmId,
    /// 目标节点（调度后确定）
    #[serde(default)]
    pub node_id: Option<NodeId>,
    /// 步骤列表
    pub steps: Vec<OperationStep>,
    /// 当前执行到的步骤索引
    pub current_step: usize,
    /// 操作状态
    pub status: OperationStatus,
    /// 创建时间（毫秒）
    pub created_at_ms: i64,
    /// 开始时间（毫秒）
    #[serde(default)]
    pub started_at_ms: Option<i64>,
    /// 结束时间（毫秒）
    #[serde(default)]
    pub finished_at_ms: Option<i64>,
    /// 错误信息
    #[serde(default)]
    pub error: Option<String>,
    /// 优先级
    #[serde(default)]
    pub priority: i32,
}

impl Operation {
    /// 创建新操作
    pub fn new(kind: OperationKind, vm_id: VmId, steps: Vec<OperationStep>) -> Self {
        let now = crate::now_ms();
        Self {
            id: OperationId::new(0), // 由存储层分配
            kind,
            vm_id,
            node_id: None,
            steps,
            current_step: 0,
            status: OperationStatus::Pending,
            created_at_ms: now,
            started_at_ms: None,
            finished_at_ms: None,
            error: None,
            priority: 0,
        }
    }

    /// 获取当前待执行步骤
    pub fn current_step_mut(&mut self) -> Option<&mut OperationStep> {
        if self.current_step < self.steps.len() {
            Some(&mut self.steps[self.current_step])
        } else {
            None
        }
    }

    /// 推进到下一步
    pub fn advance_step(&mut self) -> bool {
        self.current_step += 1;
        self.current_step < self.steps.len()
    }

    /// 是否所有步骤完成
    pub fn is_complete(&self) -> bool {
        self.current_step >= self.steps.len()
    }

    /// 标记失败
    pub fn mark_failed(&mut self, error: String) {
        self.status = OperationStatus::Failed;
        self.error = Some(error);
        self.finished_at_ms = Some(crate::now_ms());
    }

    /// 标记成功
    pub fn mark_succeeded(&mut self) {
        self.status = OperationStatus::Succeeded;
        self.finished_at_ms = Some(crate::now_ms());
    }

    /// 标记运行中
    pub fn mark_running(&mut self) {
        self.status = OperationStatus::Running;
        if self.started_at_ms.is_none() {
            self.started_at_ms = Some(crate::now_ms());
        }
    }
}

/// 当前时间毫秒
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_step_flow() {
        let mut op = Operation::new(
            OperationKind::CreateVm,
            VmId(1),
            vec![
                OperationStep { kind: StepKind::Control, name: "allocate_ip".into(), params: serde_json::json!({}), idempotency_key: Some("alloc-ip-1".into()), ..Default::default() },
                OperationStep { kind: StepKind::Node, name: "create_vm".into(), params: serde_json::json!({}), idempotency_key: Some("create-vm-1".into()), ..Default::default() },
                OperationStep { kind: StepKind::Wait, name: "wait_ready".into(), params: serde_json::json!({}), ..Default::default() },
            ],
        );

        assert_eq!(op.current_step, 0);
        assert!(!op.is_complete());

        // 执行第一步
        let step = op.current_step_mut().unwrap();
        step.status = StepStatus::Succeeded;
        assert!(op.advance_step());

        // 执行第二步
        let step = op.current_step_mut().unwrap();
        step.status = StepStatus::Succeeded;
        assert!(op.advance_step());

        // 执行第三步
        let step = op.current_step_mut().unwrap();
        step.status = StepStatus::Succeeded;
        assert!(!op.advance_step()); // 没有下一步了

        assert!(op.is_complete());
        op.mark_succeeded();
        assert_eq!(op.status, OperationStatus::Succeeded);
    }
}