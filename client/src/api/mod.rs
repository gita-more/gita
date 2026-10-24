//! Typed REST client for the Gita server.
//!
//! `GitaClient` is the single HTTP path. `request()` attaches the Bearer
//! token, sends the request, parses the error envelope
//! `{"error":{"message","status"}}` into `ApiClientError`, and deserializes
//! success bodies into `T`.

use anyhow::{Context, Result};
use reqwest::{header, Client, Method};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod endpoints;
pub mod types;
pub use endpoints::*;
pub use types::*;

/// Error returned by the client, mirroring the server's error envelope.
#[derive(Debug, thiserror::Error)]
#[error("API {status}: {message}")]
pub struct ApiClientError {
    pub status: u16,
    pub message: String,
}

#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: EnvelopeBody,
}

#[derive(Debug, Deserialize)]
struct EnvelopeBody {
    message: String,
    #[serde(default)]
    status: Option<u16>,
}

/// Typed client for the Gita REST API.
#[derive(Clone)]
pub struct GitaClient {
    pub base_url: String,
    pub http: Client,
    pub token: Option<String>,
}

impl GitaClient {
    pub fn new(base_url: String, token: Option<String>) -> Result<Self> {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .context("build reqwest client")?;
        Ok(Self {
            base_url,
            http,
            token,
        })
    }

    /// Execute a request against `{base_url}{path}`.
    pub async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<T> {
        let method_str = method.to_string();
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let mut req = self.http.request(method, &url);
        if let Some(b) = body {
            req = req.header(header::CONTENT_TYPE, "application/json").json(b);
        }
        if let Some(token) = &self.token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }

        let res = req
            .send()
            .await
            .with_context(|| format!("send {method_str} {url}"))?;
        let status = res.status();

        if status.is_success() {
            let body = res.json::<T>().await.context("deserialize response")?;
            Ok(body)
        } else {
            // Parse the server error envelope.
            let message = match res.json::<ErrorEnvelope>().await {
                Ok(env) => format!(
                    "{} (status {})",
                    env.error.message,
                    env.error.status.unwrap_or(status.as_u16())
                ),
                Err(_) => status.to_string(),
            };
            return Err(ApiClientError {
                status: status.as_u16(),
                message,
            }
            .into());
        }
    }
}
