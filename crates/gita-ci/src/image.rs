//! CI image management client.

use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterImageRequest {
    pub name: String,
    pub pool: String,
    pub path: String,
    pub arch: String,
    #[serde(rename = "kind")]
    pub kind_str: String,
    pub digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ImageInfo {
    pub id: String,
    pub name: String,
    pub pool: String,
    pub size_bytes: u64,
    pub digest: String,
    pub kind: String,
    pub path: Option<String>,
    pub arch: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ImageListResponse {
    images: Vec<ImageInfo>,
}

pub struct ImageClient {
    client: Client,
    base_url: String,
}

impl ImageClient {
    pub fn new(control_url: &str) -> Self {
        Self {
            client: Client::new(),
            base_url: control_url.trim_end_matches('/').to_string(),
        }
    }

    pub async fn register(&self, req: RegisterImageRequest) -> Result<ImageInfo> {
        let resp = self
            .client
            .post(&format!("{}/api/v1/images", self.base_url))
            .json(&req)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("register image failed ({}): {}", status, body);
        }
        Ok(serde_json::from_str(&body)?)
    }

    pub async fn list(&self) -> Result<Vec<ImageInfo>> {
        let resp = self
            .client
            .get(&format!("{}/api/v1/images", self.base_url))
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("list images failed ({}): {}", status, body);
        }
        let list: ImageListResponse = serde_json::from_str(&body)?;
        Ok(list.images)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_request_serializes_correctly() {
        let req = RegisterImageRequest {
            name: "test".into(),
            pool: "pool1".into(),
            path: "/data/test.ext4".into(),
            arch: "x86_64".into(),
            kind_str: "rootfs".into(),
            digest: "abc123".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("rootfs"));
    }
}