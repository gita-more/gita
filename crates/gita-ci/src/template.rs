//! VM template management client.

use serde::{Deserialize, Serialize};

/// 镜像引用：名字 + 可选摘要（与 rcluster `ImageRef` 对应）。
#[derive(Debug, Serialize, Deserialize)]
pub struct ImageRef {
    pub name: String,
    #[serde(default)]
    pub digest: Option<String>,
}

/// 网络挂载描述（与 rcluster `NetworkAttachment` 对应）。
#[derive(Debug, Serialize, Deserialize)]
pub struct NetworkAttachment {
    pub network: String,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub mac: Option<String>,
}

/// 虚机规格（与 rcluster `VmSpec` 的请求侧子集对应）。
#[derive(Debug, Serialize, Deserialize)]
pub struct VmSpec {
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    /// 根盘镜像（rootfs）。
    pub image: ImageRef,
    /// 内核镜像引用；`None` = 使用平台默认内核。
    #[serde(default)]
    pub kernel: Option<ImageRef>,
    /// 网络挂载。
    #[serde(default)]
    pub networks: Vec<NetworkAttachment>,
    /// VMM 后端。
    pub backend: String,
    /// 要求的宿主架构。
    #[serde(default)]
    pub arch: Option<String>,
    /// 用户数据（注入 guest 的 metadata 服务）。
    #[serde(default)]
    pub user_data: Option<String>,
}

/// 创建模板请求体（与 rcluster `CreateTemplateRequest` 对应）。
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTemplateRequest {
    pub name: String,
    pub spec: VmSpec,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TemplateInfo {
    pub id: String,
    pub name: String,
    pub version: u64,
    pub spec: serde_json::Value,
    pub created_at_ms: i64,
    pub image_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TemplateListResponse {
    templates: Vec<TemplateInfo>,
}

pub struct TemplateClient {
    client: reqwest::Client,
    base_url: String,
    /// rcluster 的 API key；启用鉴权时必填。
    api_key: Option<String>,
}

impl TemplateClient {
    pub fn new(control_url: &str) -> Self {
        Self::with_key(control_url, None)
    }

    /// 带 API key 的构造。`key` 为 `None` 时回退到环境变量
    /// `RCLUSTER_API_KEY`；仍为空则不带认证头。
    pub fn with_key(control_url: &str, key: Option<&str>) -> Self {
        let api_key = key
            .map(|k| k.to_string())
            .or_else(|| std::env::var("RCLUSTER_API_KEY").ok())
            .filter(|k| !k.is_empty());
        Self {
            client: reqwest::Client::new(),
            base_url: control_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }

    /// 给请求附加 Bearer 认证头（若配置了 key）。
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let req = self.client.request(method, &format!("{}{}", self.base_url, path));
        match &self.api_key {
            Some(key) => req.bearer_auth(key),
            None => req,
        }
    }

    pub async fn create(&self, req: CreateTemplateRequest) -> anyhow::Result<TemplateInfo> {
        let resp = self
            .request(reqwest::Method::POST, "/api/v1/templates")
            .json(&req)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("create template failed ({}): {}", status, body);
        }
        Ok(serde_json::from_str(&body)?)
    }

    pub async fn list(&self) -> anyhow::Result<Vec<TemplateInfo>> {
        let resp = self
            .request(reqwest::Method::GET, "/api/v1/templates")
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("list templates failed ({}): {}", status, body);
        }
        let list: TemplateListResponse = serde_json::from_str(&body)?;
        Ok(list.templates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_request_serializes() {
        let req = CreateTemplateRequest {
            name: "test".into(),
            spec: VmSpec {
                cpu_millis: 4000,
                memory_bytes: 4_000_000_000,
                disk_bytes: 20_000_000_000,
                image: ImageRef {
                    name: "ubuntu-24.04".into(),
                    digest: None,
                },
                kernel: None,
                networks: vec![],
                backend: "qemu".into(),
                arch: None,
                user_data: None,
            },
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"spec\""));
        assert!(json.contains("cpu_millis"));
        assert!(json.contains("ubuntu-24.04"));
    }

    #[test]
    fn supports_kernel_reference() {
        let req = CreateTemplateRequest {
            name: "test".into(),
            spec: VmSpec {
                cpu_millis: 4000,
                memory_bytes: 4_000_000_000,
                disk_bytes: 20_000_000_000,
                image: ImageRef {
                    name: "ubuntu-24.04".into(),
                    digest: None,
                },
                kernel: Some(ImageRef {
                    name: "vmlinux".into(),
                    digest: None,
                }),
                networks: vec![],
                backend: "qemu".into(),
                arch: None,
                user_data: None,
            },
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"kernel\""));
        assert!(json.contains("vmlinux"));
    }
}