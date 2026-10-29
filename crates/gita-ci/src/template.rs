//! VM template management client.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTemplateRequest {
    pub name: String,
    pub image: String,
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub network: String,
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
    base_url: String,
}

impl TemplateClient {
    pub fn new(control_url: &str) -> Self {
        Self {
            base_url: control_url.trim_end_matches('/').to_string(),
        }
    }

    pub async fn create(&self, req: CreateTemplateRequest) -> anyhow::Result<TemplateInfo> {
        let resp = reqwest::Client::new()
            .post(&format!("{}/api/v1/templates", self.base_url))
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
        let resp = reqwest::Client::new()
            .get(&format!("{}/api/v1/templates", self.base_url))
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
            image: "img".into(),
            cpu_millis: 4000,
            memory_bytes: 4_000_000_000,
            disk_bytes: 20_000_000_000,
            network: "net1".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("cpu_millis"));
    }
}