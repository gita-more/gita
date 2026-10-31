//! 节点代理主循环：注册 → 心跳 → 长轮询操作 → 执行 → 上报

use crate::config::NodeConfig;
use anyhow::{Context, Result};
use gita_vm_api::node::{NodePhase, NodeStatus};
use gita_vm_driver_vmm::{create_driver, VmmDriver};
use std::time::Duration;

/// 节点代理
pub struct NodeAgent {
    config: NodeConfig,
    http: reqwest::Client,
    node_id: Option<i64>,
    drivers: std::collections::HashMap<gita_vm_api::node::VmmKind, Box<dyn VmmDriver>>,
}

impl NodeAgent {
    pub fn new(config: NodeConfig) -> Self {
        let mut drivers = std::collections::HashMap::new();
        for kind in config.vmm_kinds() {
            drivers.insert(kind, create_driver(kind));
        }
        Self {
            config,
            http: reqwest::Client::new(),
            node_id: None,
            drivers,
        }
    }

    /// 注册节点到控制面
    pub async fn register(&mut self) -> Result<i64> {
        let name = self.config.node_name_or_hostname();
        let vmm: Vec<String> = self
            .config
            .vmm_kinds()
            .iter()
            .map(|k| k.as_str().to_string())
            .collect();
        let arch = vec![std::env::consts::ARCH.to_string()];

        let resp = self
            .http
            .post(format!("{}/api/v1/vm/internal/nodes", self.config.control_url))
            .header("x-api-key", &self.config.api_key)
            .json(&serde_json::json!({
                "name": name,
                "cpu_millis": self.config.cpu_millis,
                "memory_bytes": self.config.memory_bytes,
                "disk_bytes": self.config.disk_bytes,
                "vmm": vmm,
                "arch": arch,
                "labels": {},
            }))
            .send()
            .await
            .context("register node")?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;
        if !status.is_success() {
            anyhow::bail!("register failed ({}): {}", status, body);
        }
        let node_id = body["node_id"].as_i64().context("missing node_id")?;
        self.node_id = Some(node_id);
        tracing::info!(node_id, name = %name, "node registered");
        Ok(node_id)
    }

    /// 主循环：心跳 + 拉取操作
    pub async fn run(&mut self) -> Result<()> {
        if self.node_id.is_none() {
            self.register().await?;
        }
        let node_id = self.node_id.unwrap();

        let poll_interval = Duration::from_secs(self.config.poll_interval_secs);
        let mut ticker = tokio::time::interval(poll_interval);

        loop {
            ticker.tick().await;

            // 心跳
            if let Err(e) = self.heartbeat(node_id).await {
                tracing::warn!(error = %e, "heartbeat failed");
            }

            // 拉取并执行操作
            if let Err(e) = self.poll_and_execute(node_id).await {
                tracing::warn!(error = %e, "poll operations failed");
            }
        }
    }

    async fn heartbeat(&self, node_id: i64) -> Result<()> {
        let resp = self
            .http
            .post(format!(
                "{}/api/v1/vm/internal/nodes/{}/heartbeat",
                self.config.control_url, node_id
            ))
            .header("x-api-key", &self.config.api_key)
            .json(&NodeStatus {
                alloc_cpu_millis: 0,
                alloc_memory_bytes: 0,
                alloc_disk_bytes: 0,
                running_vms: 0,
                phase: NodePhase::Online,
                last_heartbeat_ms: gita_vm_core_now(),
            })
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("heartbeat returned {}", resp.status());
        }
        Ok(())
    }

    async fn poll_and_execute(&mut self, node_id: i64) -> Result<()> {
        let resp = self
            .http
            .get(format!(
                "{}/api/v1/vm/internal/nodes/{}/operations/next",
                self.config.control_url, node_id
            ))
            .header("x-api-key", &self.config.api_key)
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("poll returned {}", resp.status());
        }
        let body: serde_json::Value = resp.json().await?;
        let ops = body["operations"].as_array().cloned().unwrap_or_default();

        for op_value in ops {
            let op: gita_vm_api::operation::Operation = match serde_json::from_value(op_value) {
                Ok(o) => o,
                Err(e) => {
                    tracing::warn!(error = %e, "parse operation failed");
                    continue;
                }
            };
            tracing::info!(op = %op.id, kind = ?op.kind, "executing operation");
            match self.execute_operation(&op).await {
                Ok(()) => self.report(node_id, op.id.0, true, None).await?,
                Err(e) => self.report(node_id, op.id.0, false, Some(e.to_string())).await?,
            }
        }
        Ok(())
    }

    /// 执行操作中的节点步骤
    async fn execute_operation(&mut self, op: &gita_vm_api::operation::Operation) -> Result<()> {
        // 从 VM spec 中获取 VMM 类型（操作参数里有 vm_id，这里简化为从参数取）
        for step in &op.steps {
            match step.kind {
                gita_vm_api::operation::StepKind::Node => {
                    self.execute_node_step(&step.name, &step.params).await?;
                }
                gita_vm_api::operation::StepKind::Control | gita_vm_api::operation::StepKind::Wait => {
                    // 控制面步骤，跳过
                }
                gita_vm_api::operation::StepKind::Compensate => {}
            }
        }
        Ok(())
    }

    /// 执行单个节点步骤
    async fn execute_node_step(&mut self, name: &str, params: &serde_json::Value) -> Result<()> {
        let vm_id = params["vm_id"].as_str().unwrap_or("unknown").to_string();

        // 取 qemu 驱动（当前唯一实现）
        let driver = self
            .drivers
            .get_mut(&gita_vm_api::node::VmmKind::Qemu)
            .ok_or_else(|| anyhow::anyhow!("no qemu driver"))?;

        match name {
            "create_disk" | "define_vm" => {
                // 需要 spec：从参数重建（简化）
                tracing::info!(step = %name, vm = %vm_id, "node step (stub)");
            }
            "start_vm" => {
                driver.start_vm(&vm_id).await?;
            }
            "stop_vm" => {
                driver.stop_vm(&vm_id, true).await?;
            }
            "undefine_vm" | "delete_disk" => {
                driver.delete_vm(&vm_id).await?;
            }
            _ => {
                tracing::warn!(step = %name, "unknown node step");
            }
        }
        Ok(())
    }

    async fn report(&self, node_id: i64, op_id: i64, success: bool, error: Option<String>) -> Result<()> {
        let resp = self
            .http
            .post(format!(
                "{}/api/v1/vm/internal/nodes/{}/operations/report",
                self.config.control_url, node_id
            ))
            .header("x-api-key", &self.config.api_key)
            .json(&serde_json::json!({
                "operation_id": op_id,
                "success": success,
                "error": error,
                "output": {},
            }))
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("report returned {}", resp.status());
        }
        Ok(())
    }
}

fn gita_vm_core_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}