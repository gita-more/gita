//! 节点代理主循环：注册 → 心跳 → 长轮询操作 → 执行 → 上报

use crate::config::NodeConfig;
use anyhow::{Context, Result};
use gita_vm_api::node::{NodePhase, NodeStatus};
use gita_vm_driver_vmm::{create_driver, VmmDriver};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct NodeCredential {
    control_url: String,
    node_id: i64,
    node_secret: String,
}

/// 节点代理
pub struct NodeAgent {
    config: NodeConfig,
    http: reqwest::Client,
    node_id: Option<i64>,
    node_secret: Option<String>,
    drivers: std::collections::HashMap<gita_vm_api::node::VmmKind, Box<dyn VmmDriver>>,
}

impl NodeAgent {
    pub fn new(config: NodeConfig) -> Self {
        let mut drivers = std::collections::HashMap::new();
        for kind in config.vmm_kinds() {
            drivers.insert(kind, create_driver(kind));
        }
        Self::with_drivers(config, drivers)
    }

    /// An agent that drives the given backends.
    ///
    /// The backends are a parameter so that the agent can be tested
    /// against a stand-in for QEMU: the agent's own behaviour is the
    /// conversation with the control plane, and that conversation must
    /// be exercised without a hypervisor.
    pub fn with_drivers(
        config: NodeConfig,
        drivers: std::collections::HashMap<gita_vm_api::node::VmmKind, Box<dyn VmmDriver>>,
    ) -> Self {
        Self {
            config,
            http: reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            node_id: None,
            node_secret: None,
            drivers,
        }
    }

    fn credential_path(&self) -> PathBuf {
        Path::new(&self.config.work_dir).join("node-credential.json")
    }

    fn load_credential(&self) -> Option<NodeCredential> {
        let path = self.credential_path();
        let raw = std::fs::read_to_string(&path).ok()?;
        match serde_json::from_str::<NodeCredential>(&raw) {
            Ok(cred) if cred.control_url == self.config.control_url => Some(cred),
            Ok(_) => None,
            Err(_) => None,
        }
    }

    fn save_credential(&self, node_id: i64, node_secret: &str) -> Result<()> {
        let path = self.credential_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create work dir {}", dir.display()))?;
        }
        let credential = NodeCredential {
            control_url: self.config.control_url.clone(),
            node_id,
            node_secret: node_secret.to_string(),
        };
        let json = serde_json::to_vec_pretty(&credential)?;
        std::fs::write(&path, json).with_context(|| format!("write node credential {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("chmod 0600 {}", path.display()))?;
        }
        Ok(())
    }

    fn with_auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut request = request;
        if !self.config.api_key.is_empty() {
            request = request.header("x-api-key", &self.config.api_key);
        }
        if let Some(secret) = self.node_secret.as_deref() {
            request = request.header("x-node-secret", secret);
        }
        request
    }

    fn registration_body(&self) -> serde_json::Value {
        let name = self.config.node_name_or_hostname();
        let vmm: Vec<String> = self.config.vmm_kinds().iter().map(|k| k.as_str().to_string()).collect();
        let arch = vec![std::env::consts::ARCH.to_string()];
        serde_json::json!({
            "name": name,
            "cpu_millis": self.config.cpu_millis,
            "memory_bytes": self.config.memory_bytes,
            "disk_bytes": self.config.disk_bytes,
            "vmm": vmm,
            "arch": arch,
            "labels": {},
        })
    }

    pub async fn register(&mut self) -> Result<i64> {
        if let Some(cred) = self.load_credential() {
            self.node_id = Some(cred.node_id);
            self.node_secret = Some(cred.node_secret);
            return Ok(cred.node_id);
        }
        let join_token = self.config.join_token.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        if let Some(token) = join_token {
            return self.enroll(&token).await;
        }
        self.register_legacy().await
    }

    async fn enroll(&mut self, join_token: &str) -> Result<i64> {
        let name = self.config.node_name_or_hostname();
        let resp = self.http
            .post(format!("{}/api/v1/vm/internal/nodes/enroll", self.config.control_url))
            .header("x-join-token", join_token)
            .json(&self.registration_body())
            .send().await.context("enroll node")?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;
        if !status.is_success() { anyhow::bail!("enroll failed ({}): {}", status, body); }
        let node_id = body["node_id"].as_i64().context("missing node_id")?;
        let secret = body["node_secret"].as_str().context("missing node_secret")?.to_string();
        self.node_id = Some(node_id);
        self.node_secret = Some(secret.clone());
        self.save_credential(node_id, &secret)?;
        tracing::info!(node_id, name = %name, "node enrolled");
        Ok(node_id)
    }

    async fn register_legacy(&mut self) -> Result<i64> {
        let name = self.config.node_name_or_hostname();
        let resp = self.with_auth(self.http.post(format!("{}/api/v1/vm/internal/nodes", self.config.control_url)))
            .json(&self.registration_body())
            .send().await.context("register node")?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;
        if !status.is_success() { anyhow::bail!("register failed ({}): {}", status, body); }
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

    /// Tell the control plane the node is alive.
    async fn heartbeat(&self, node_id: i64) -> Result<()> {
        let resp = self
            .with_auth(self.http.post(format!(
                "{}/api/v1/vm/internal/nodes/{}/heartbeat",
                self.config.control_url, node_id
            )))
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

    /// Fetch the operations waiting for this node and run them.
    async fn poll_and_execute(&mut self, node_id: i64) -> Result<()> {
        let resp = self
            .with_auth(self.http.get(format!(
                "{}/api/v1/vm/internal/nodes/{}/operations/next",
                self.config.control_url, node_id
            )))
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
            .with_auth(self.http.post(format!(
                "{}/api/v1/vm/internal/nodes/{}/operations/report",
                self.config.control_url, node_id
            )))
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

#[cfg(test)]
mod tests {
    //! The agent's conversation with the control plane.
    //!
    //! The agent is a client: it registers, it beats, it pulls the
    //! operations that are waiting for it and it reports what happened.
    //! A mock control plane is what makes that conversation testable
    //! without a hypervisor on the other end.

    use super::*;
    use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, routing::post, Json, Router};
    use gita_vm_api::node::VmmKind;
    use gita_vm_api::operation::{Operation, OperationKind, OperationStep, StepKind};
    use gita_vm_api::vm::VmSpec;
    use std::sync::{Arc, Mutex};

    /// What the control plane saw.
    #[derive(Clone, Debug)]
    struct Seen {
        path: String,
        api_key: Option<String>,
        join_token: Option<String>,
        node_secret: Option<String>,
        body: serde_json::Value,
    }

    fn seen_from_headers(
        path: String,
        headers: &axum::http::HeaderMap,
        body: serde_json::Value,
    ) -> Seen {
        Seen {
            path,
            api_key: headers.get("x-api-key").and_then(|v| v.to_str().ok()).map(str::to_string),
            join_token: headers.get("x-join-token").and_then(|v| v.to_str().ok()).map(str::to_string),
            node_secret: headers.get("x-node-secret").and_then(|v| v.to_str().ok()).map(str::to_string),
            body,
        }
    }

    /// The mock control plane: it answers like the real one does and
    /// remembers what it was asked.
    #[derive(Clone)]
    struct MockControlPlane {
        seen: Arc<Mutex<Vec<Seen>>>,
        /// The operations the next poll hands out.
        ///
        /// They are raw JSON rather than typed operations: the agent
        /// has to survive an operation it cannot parse, and that can
        /// only be shown by handing out something that is not one.
        operations: Arc<Mutex<Vec<serde_json::Value>>>,
        /// The node id a registration is answered with.
        node_id: i64,
        /// Whether the registration is refused.
        refuse_registration: bool,
    }

    impl MockControlPlane {
        fn new() -> Self {
            Self {
                seen: Arc::new(Mutex::new(Vec::new())),
                operations: Arc::new(Mutex::new(Vec::new())),
                node_id: 42,
                refuse_registration: false,
            }
        }

        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    /// Remember a call: where it went, what key it carried and what it
    /// said.
    async fn remember(
        State(plane): State<MockControlPlane>,
        uri: axum::http::Uri,
        headers: axum::http::HeaderMap,
        body: Option<Json<serde_json::Value>>,
    ) -> Json<serde_json::Value> {
        plane.seen.lock().unwrap().push(seen_from_headers(
            uri.path().to_string(),
            &headers,
            body.map(|Json(b)| b).unwrap_or(serde_json::Value::Null),
        ));
        Json(serde_json::json!({}))
    }

    async fn register(
        State(plane): State<MockControlPlane>,
        headers: axum::http::HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> impl IntoResponse {
        plane.seen.lock().unwrap().push(seen_from_headers(
            "register".to_string(),
            &headers,
            body,
        ));
        if plane.refuse_registration {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "no room" })),
            )
                .into_response();
        }
        Json(serde_json::json!({ "node_id": plane.node_id })).into_response()
    }

    async fn enroll(
        State(plane): State<MockControlPlane>,
        headers: axum::http::HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> impl IntoResponse {
        plane.seen.lock().unwrap().push(seen_from_headers(
            "enroll".to_string(),
            &headers,
            body,
        ));
        Json(serde_json::json!({
            "node_id": plane.node_id,
            "node_secret": "the-node-secret",
        }))
        .into_response()
    }

    async fn next_operations(State(plane): State<MockControlPlane>) -> Json<serde_json::Value> {
        let ops = plane.operations.lock().unwrap().clone();
        Json(serde_json::json!({ "operations": ops }))
    }

    /// Serve the mock and hand back its address.
    async fn serve(plane: MockControlPlane) -> (String, tokio::task::JoinHandle<()>) {
        let router = Router::new()
            .route("/api/v1/vm/internal/nodes", post(register))
            .route("/api/v1/vm/internal/nodes/enroll", post(enroll))
            .route("/api/v1/vm/internal/nodes/{id}/heartbeat", post(remember))
            .route("/api/v1/vm/internal/nodes/{id}/operations/next", get(next_operations))
            .route("/api/v1/vm/internal/nodes/{id}/operations/report", post(remember))
            .with_state(plane);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{addr}"), handle)
    }

    fn config(control_url: &str) -> NodeConfig {
        NodeConfig {
            control_url: control_url.to_string(),
            node_name: Some("worker-1".to_string()),
            api_key: "the-node-key".to_string(),
            join_token: None,
            work_dir: "/var/lib/gita/vm".to_string(),
            cpu_millis: 8000,
            memory_bytes: 16_000_000_000,
            disk_bytes: 100_000_000_000,
            vmm: "qemu".to_string(),
            poll_interval_secs: 10,
        }
    }

    /// An operation that starts a VM, in the shape the control plane
    /// sends it.
    fn start_vm_operation(id: i64) -> serde_json::Value {
        serde_json::to_value(Operation {
            id: gita_vm_api::ids::OperationId(id),
            kind: OperationKind::StartVm,
            vm_id: gita_vm_api::ids::VmId(3),
            node_id: Some(gita_vm_api::ids::NodeId(42)),
            steps: vec![OperationStep {
                kind: StepKind::Node,
                name: "start_vm".into(),
                params: serde_json::json!({ "vm_id": "3" }),
                ..Default::default()
            }],
            current_step: 0,
            status: gita_vm_api::operation::OperationStatus::Pending,
            created_at_ms: 0,
            started_at_ms: None,
            finished_at_ms: None,
            error: None,
            priority: 0,
        })
        .unwrap()
    }

    /// A driver that remembers what it was asked to do.
    struct FakeDriver {
        started: Arc<Mutex<Vec<String>>>,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl VmmDriver for FakeDriver {
        fn kind(&self) -> VmmKind {
            VmmKind::Qemu
        }
        async fn define_vm(&self, _vm_id: &str, _spec: &VmSpec, _work_dir: &std::path::Path) -> gita_vm_driver_vmm::Result<()> {
            Ok(())
        }
        async fn start_vm(&self, vm_id: &str) -> gita_vm_driver_vmm::Result<()> {
            self.started.lock().unwrap().push(vm_id.to_string());
            if self.fail {
                return Err(gita_vm_driver_vmm::Error::OperationFailed("no memory".into()));
            }
            Ok(())
        }
        async fn stop_vm(&self, _vm_id: &str, _force: bool) -> gita_vm_driver_vmm::Result<()> {
            Ok(())
        }
        async fn delete_vm(&self, _vm_id: &str) -> gita_vm_driver_vmm::Result<()> {
            Ok(())
        }
        async fn get_vm_status(&self, _vm_id: &str) -> gita_vm_driver_vmm::Result<gita_vm_driver_vmm::VmPowerState> {
            Ok(gita_vm_driver_vmm::VmPowerState::Running)
        }
        async fn get_console(&self, _vm_id: &str) -> gita_vm_driver_vmm::Result<Option<String>> {
            Ok(None)
        }
        async fn hotplug(&self, _vm_id: &str, _cpu: Option<u64>, _memory: Option<u64>) -> gita_vm_driver_vmm::Result<()> {
            Ok(())
        }
        async fn resize_disk(&self, _vm_id: &str, _new_size: u64) -> gita_vm_driver_vmm::Result<()> {
            Ok(())
        }
        async fn create_snapshot(&self, _vm_id: &str, _path: &std::path::Path) -> gita_vm_driver_vmm::Result<()> {
            Ok(())
        }
        async fn restore_snapshot(&self, _vm_id: &str, _path: &std::path::Path) -> gita_vm_driver_vmm::Result<()> {
            Ok(())
        }
    }

    fn agent_with(
        config: NodeConfig,
        started: Arc<Mutex<Vec<String>>>,
        fail: bool,
    ) -> NodeAgent {
        let mut drivers = std::collections::HashMap::new();
        drivers.insert(
            VmmKind::Qemu,
            Box::new(FakeDriver { started, fail }) as Box<dyn VmmDriver>,
        );
        NodeAgent::with_drivers(config, drivers)
    }

    /// The registration tells the control plane what this node is and
    /// comes back with the id the control plane knows it by.
    #[tokio::test]
    async fn the_registration_says_what_the_node_is() {
        let plane = MockControlPlane::new();
        let (url, _server) = serve(plane.clone()).await;

        let mut agent = agent_with(config(&url), Arc::new(Mutex::new(Vec::new())), false);
        let node_id = agent.register().await.unwrap();
        assert_eq!(node_id, 42);

        let seen = plane.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].path, "register");
        // The node proves itself with the key the deployment gave it.
        assert_eq!(seen[0].api_key.as_deref(), Some("the-node-key"));
        assert_eq!(seen[0].body["name"], "worker-1");
        assert_eq!(seen[0].body["cpu_millis"], 8000);
        assert_eq!(seen[0].body["vmm"][0], "qemu");
        assert_eq!(seen[0].body["arch"][0], std::env::consts::ARCH);
    }


    /// A registration the control plane refuses is an error, not a
    /// silent success: the node must not think it is known.
    #[tokio::test]
    async fn a_refused_registration_is_an_error() {
        let mut plane = MockControlPlane::new();
        plane.refuse_registration = true;
        let (url, _server) = serve(plane).await;

        let mut agent = agent_with(config(&url), Arc::new(Mutex::new(Vec::new())), false);
        let err = agent.register().await.unwrap_err().to_string();
        assert!(err.contains("500"), "got {err}");
    }

    /// The heartbeat carries the node's key as well.
    #[tokio::test]
    async fn the_heartbeat_carries_the_node_key() {
        let plane = MockControlPlane::new();
        let (url, _server) = serve(plane.clone()).await;

        let agent = agent_with(config(&url), Arc::new(Mutex::new(Vec::new())), false);
        agent.heartbeat(42).await.unwrap();

        let seen = plane.seen();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].path.contains("heartbeat"), "got {}", seen[0].path);
        assert_eq!(seen[0].api_key.as_deref(), Some("the-node-key"));
        assert_eq!(seen[0].body["phase"], "online");
    }

    /// An operation the control plane hands out is run on the node and
    /// then reported back.
    #[tokio::test]
    async fn an_operation_is_run_and_reported() {
        let plane = MockControlPlane::new();
        let started = Arc::new(Mutex::new(Vec::new()));
        *plane.operations.lock().unwrap() = vec![start_vm_operation(7)];
        let (url, _server) = serve(plane.clone()).await;

        let mut agent = agent_with(config(&url), started.clone(), false);
        agent.poll_and_execute(42).await.unwrap();

        // The node started the VM the operation names.
        assert_eq!(started.lock().unwrap().as_slice(), ["3"]);

        let seen = plane.seen();
        let report = seen.iter().find(|s| s.path.contains("report")).unwrap();
        assert_eq!(report.body["operation_id"], 7);
        assert_eq!(report.body["success"], true);
        assert!(report.body["error"].is_null(), "got {}", report.body);
    }

    /// An operation that fails on the node is reported as failed, with
    /// the reason — the control plane has to know why it did not happen.
    #[tokio::test]
    async fn a_failed_operation_is_reported_as_failed() {
        let plane = MockControlPlane::new();
        *plane.operations.lock().unwrap() = vec![start_vm_operation(8)];
        let (url, _server) = serve(plane.clone()).await;

        let mut agent = agent_with(config(&url), Arc::new(Mutex::new(Vec::new())), true);
        agent.poll_and_execute(42).await.unwrap();

        let seen = plane.seen();
        let report = seen.iter().find(|s| s.path.contains("report")).unwrap();
        assert_eq!(report.body["success"], false);
        assert!(
            report.body["error"].as_str().unwrap_or("").contains("no memory"),
            "got {}",
            report.body
        );
    }

    /// An operation the node cannot make sense of is skipped, not
    /// fatal: one bad operation must not stop the node from working.
    /// The operation after it is still run.
    #[tokio::test]
    async fn an_operation_that_makes_no_sense_is_skipped() {
        let plane = MockControlPlane::new();
        let started = Arc::new(Mutex::new(Vec::new()));
        *plane.operations.lock().unwrap() = vec![
            serde_json::json!({ "id": "not-a-number", "kind": "start_vm" }),
            start_vm_operation(9),
        ];
        let (url, _server) = serve(plane.clone()).await;

        let mut agent = agent_with(config(&url), started.clone(), false);
        agent.poll_and_execute(42).await.unwrap();

        // The good operation after the broken one still ran.
        assert_eq!(started.lock().unwrap().as_slice(), ["3"]);

        // And only that one was reported.
        let seen = plane.seen();
        let reports: Vec<_> = seen.iter().filter(|s| s.path.contains("report")).collect();
        assert_eq!(reports.len(), 1, "got {reports:?}");
        assert_eq!(reports[0].body["operation_id"], 9);
    }

    /// The node key is sent as a header and never in the body: a body
    /// is the part that ends up in logs.
    #[tokio::test]
    async fn the_node_key_travels_in_the_header() {
        let plane = MockControlPlane::new();
        let (url, _server) = serve(plane.clone()).await;

        let mut agent = agent_with(config(&url), Arc::new(Mutex::new(Vec::new())), false);
        agent.register().await.unwrap();

        let seen = plane.seen();
        assert!(!seen[0].body.to_string().contains("the-node-key"));
    }

    fn temp_work_dir() -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("gita-vm-node-test-{stamp}"))
    }

    #[tokio::test]
    async fn a_join_token_is_exchanged_for_a_saved_credential() {
        let plane = MockControlPlane::new();
        let (url, _server) = serve(plane.clone()).await;
        let dir = temp_work_dir();

        let mut cfg = config(&url);
        cfg.api_key = String::new();
        cfg.join_token = Some("join-1".to_string());
        cfg.work_dir = dir.to_string_lossy().to_string();

        let mut agent = agent_with(cfg.clone(), Arc::new(Mutex::new(Vec::new())), false);
        assert_eq!(agent.register().await.unwrap(), 42);

        let seen = plane.seen();
        assert_eq!(seen[0].path, "enroll");
        assert_eq!(seen[0].join_token.as_deref(), Some("join-1"));
        assert!(seen[0].api_key.is_none());

        let path = dir.join("node-credential.json");
        let saved: NodeCredential =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved.node_id, 42);
        assert_eq!(saved.node_secret, "the-node-secret");

        agent.heartbeat(42).await.unwrap();
        let seen = plane.seen();
        let beat = seen.iter().find(|s| s.path.contains("heartbeat")).unwrap();
        assert_eq!(beat.node_secret.as_deref(), Some("the-node-secret"));
        assert!(beat.api_key.is_none());

        let mut restarted = agent_with(cfg, Arc::new(Mutex::new(Vec::new())), false);
        assert_eq!(restarted.register().await.unwrap(), 42);
        assert_eq!(plane.seen().len(), 2, "enroll + heartbeat, no second enroll");

        let _ = std::fs::remove_dir_all(&dir);
    }
}