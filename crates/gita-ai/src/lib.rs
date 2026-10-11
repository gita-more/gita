//! AI orchestration for Gita.
//!
//! This crate provides:
//!
//! - an abstraction over LLM providers (`LlmProvider`)
//! - an AI code review engine that analyzes a diff and produces
//!   structured, line-level comments with a risk score
//! - a semantic code search interface (embedding + vector search)
//!
//! The `LlmProvider` is intentionally small so it can be backed by any
//! OpenAI-compatible API or a local Ollama instance.

use anyhow::{anyhow, Context, Result};
use gita_core::{Review, ReviewComment, Severity};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmConfig {
    /// Base URL of the OpenAI-compatible API.
    pub base_url: Option<String>,
    /// API key.
    pub api_key: Option<String>,
    /// Model name, e.g. "gpt-4o-mini" or "qwen2.5-coder".
    pub model: Option<String>,
    /// Timeout in seconds.
    pub timeout_secs: u64,
}

impl LlmConfig {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn is_configured(&self) -> bool {
        self.base_url.is_some() && self.api_key.is_some()
    }
}

/// A trait for LLM providers.
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a chat completion request with a system prompt and user prompt.
    /// Returns the assistant's text reply.
    async fn chat(&self, system: &str, user: &str) -> Result<String>;
}

/// An LLM provider backed by any OpenAI-compatible `/chat/completions` API.
#[derive(Debug, Clone)]
pub struct OpenAiCompatibleLlm {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    timeout: Duration,
}

impl OpenAiCompatibleLlm {
    pub fn new(config: LlmConfig) -> Result<Self> {
        let base_url = config
            .base_url
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let api_key = config
            .api_key
            .ok_or_else(|| anyhow!("api_key is required for OpenAiCompatibleLlm"))?;
        let model = config.model.unwrap_or_else(|| "gpt-4o-mini".to_string());
        let timeout = Duration::from_secs(config.timeout_secs.max(10));
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .context("build reqwest client")?;
        Ok(Self {
            client,
            base_url,
            api_key,
            model,
            timeout,
        })
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiCompatibleLlm {
    async fn chat(&self, system: &str, user: &str) -> Result<String> {
        let payload = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "temperature": 0.2,
        });
        let resp = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .context("send chat request")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("LLM API returned {status}: {}", truncate(&text, 300)));
        }
        let json: serde_json::Value = serde_json::from_str(&text)
            .with_context(|| format!("invalid LLM response JSON: {}", truncate(&text, 200)))?;
        let content = json
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("LLM response missing content"))?;
        Ok(content.trim().to_string())
    }
}

/// A stub LLM provider for development/tests when no API key is available.
/// It returns a canned review so the full pipeline can be exercised.
#[derive(Debug, Clone)]
pub struct StubLlm;

#[async_trait::async_trait]
impl LlmProvider for StubLlm {
    async fn chat(&self, _system: &str, _user: &str) -> Result<String> {
        // A fixed, deterministic review for development.
        Ok(r#"{
  "summary": "This is a stub review generated because no LLM API key is configured. Configure gita.llm to enable real AI reviews.",
  "risk_score": 20,
  "comments": [
    {
      "file_path": "README.md",
      "line": null,
      "severity": "info",
      "message": "Stub review: add an LLM API key to enable real reviews.",
      "suggestion": "Set GITA_LLM_API_KEY or gita.llm.base_url / api_key."
    }
  ]
}"#
        .to_string())
    }
}

/// A parsed structured review from the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResult {
    pub summary: String,
    pub risk_score: i64,
    pub comments: Vec<ReviewCommentInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewCommentInput {
    pub file_path: String,
    pub line: Option<i64>,
    pub severity: String,
    pub message: String,
    pub suggestion: String,
}

/// Parse the LLM's JSON output into a `ReviewResult`.
pub fn parse_review_json(text: &str) -> Result<ReviewResult> {
    // Strip markdown code fences if present.
    let text = text.trim();
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .unwrap_or(text)
        .trim();
    let text = text.strip_suffix("```").unwrap_or(text).trim();
    let v: serde_json::Value = serde_json::from_str(text)
        .with_context(|| format!("invalid review JSON: {}", truncate(text, 500)))?;
    // The LLM might return { "review": {...} } or the object directly.
    let obj = v
        .get("review")
        .unwrap_or(&v)
        .clone();
    let result: ReviewResult = serde_json::from_value(obj)
        .with_context(|| format!("review JSON missing fields: {}", truncate(&v.to_string(), 500)))?;
    Ok(result)
}

/// Convert a parsed review into the core `Review` model.
pub fn review_from_result(
    repo_id: i64,
    pr_id: Option<i64>,
    commit_sha: &str,
    result: ReviewResult,
) -> Review {
    let now = chrono::Utc::now();
    let comments = result
        .comments
        .iter()
        .map(|c| ReviewComment {
            id: 0,
            review_id: 0,
            file_path: c.file_path.clone(),
            line: c.line,
            severity: match c.severity.as_str() {
                "warning" => Severity::Warning,
                "error" => Severity::Error,
                _ => Severity::Info,
            },
            message: c.message.clone(),
            suggestion: c.suggestion.clone(),
            created_at: now,
            resolved: false,
            resolved_by: None,
            resolved_at: None,
        })
        .collect();
    Review {
        id: 0,
        pr_id,
        repo_id,
        commit_sha: commit_sha.to_string(),
        status: "completed".to_string(),
        risk_score: result.risk_score.clamp(0, 100),
        summary: result.summary.clone(),
        created_at: now,
        updated_at: now,
        comments,
    }
}

/// A shared handle to the configured LLM provider.
#[derive(Clone)]
pub struct AiEngine {
    pub llm: Arc<dyn LlmProvider>,
    pub configured: bool,
}

impl AiEngine {
    /// Create an engine from config, falling back to a stub provider
    /// when no API key is configured.
    pub fn from_config(config: LlmConfig) -> Self {
        if let Ok(llm) = OpenAiCompatibleLlm::new(config.clone()) {
            Self {
                llm: Arc::new(llm),
                configured: true,
            }
        } else {
            Self {
                llm: Arc::new(StubLlm),
                configured: false,
            }
        }
    }

    pub fn stub() -> Self {
        Self {
            llm: Arc::new(StubLlm),
            configured: false,
        }
    }

    /// Run an AI code review on a unified diff.
    pub async fn review_diff(&self, diff: &str, repo_name: &str) -> Result<ReviewResult> {
        let system = "You are an expert senior software engineer performing a code review. \
            Analyze the provided unified diff carefully. Identify real bugs, security issues, \
            performance problems, and style issues. Be concise and specific. \
            Respond ONLY with a JSON object of the form: \
            {\"summary\": string, \"risk_score\": number (0-100), \
            \"comments\": [{\"file_path\": string, \"line\": number|null, \
            \"severity\": \"info\"|\"warning\"|\"error\", \
            \"message\": string, \"suggestion\": string}]}. \
            If there are no issues, return an empty comments array. Do not wrap in markdown fences.";
        let user = format!(
            "Repository: {}\n\nDiff to review:\n```diff\n{}\n```",
            repo_name,
            truncate(diff, 60_000)
        );
        let response = self
            .llm
            .chat(system, &user)
            .await
            .context("LLM review failed")?;
        parse_review_json(&response)
    }
}

/// Truncate a string to `max` chars with a marker.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

// ---------------------------------------------------------------------------
// Semantic search types (embedding-based)
// ---------------------------------------------------------------------------

/// A code chunk that has been (or will be) embedded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeChunk {
    pub repo_id: i64,
    pub file_path: String,
    pub blob_sha: String,
    pub chunk_index: usize,
    pub content: String,
    pub embedding: Vec<f32>,
}

/// A search hit returned by semantic search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub repo_id: i64,
    pub file_path: String,
    pub content: String,
    pub score: f32,
}

/// An embedding provider abstraction.
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a list of texts. Returns a list of vectors.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// An embedding provider backed by an OpenAI-compatible embeddings API.
#[derive(Debug, Clone)]
pub struct OpenAiEmbeddings {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl OpenAiEmbeddings {
    pub fn new(config: LlmConfig) -> Result<Self> {
        let base_url = config
            .base_url
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let api_key = config
            .api_key
            .ok_or_else(|| anyhow!("api_key required for OpenAiEmbeddings"))?;
        let model = config
            .model
            .unwrap_or_else(|| "text-embedding-3-small".to_string());
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs.max(10)))
            .build()?;
        Ok(Self {
            client,
            base_url,
            api_key,
            model,
        })
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for OpenAiEmbeddings {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let payload = serde_json::json!({
            "model": self.model,
            "input": texts,
        });
        let resp = self
            .client
            .post(format!("{}/embeddings", self.base_url.trim_end_matches('/')))
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .context("send embedding request")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("embedding API returned {status}: {}", truncate(&text, 300)));
        }
        let json: serde_json::Value = serde_json::from_str(&text)
            .with_context(|| format!("invalid embedding JSON: {}", truncate(&text, 200)))?;
        let data = json
            .pointer("/data")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("embedding response missing data"))?;
        let mut vecs = Vec::with_capacity(data.len());
        for item in data {
            let embedding = item
                .get("embedding")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow!("embedding missing vector"))?;
            let v: Vec<f32> = embedding
                .iter()
                .filter_map(|x| x.as_f64().map(|f| f as f32))
                .collect();
            vecs.push(v);
        }
        Ok(vecs)
    }
}

/// A stub embedding provider returning deterministic vectors (for tests).
#[derive(Debug, Clone)]
pub struct StubEmbeddings;

#[async_trait::async_trait]
impl EmbeddingProvider for StubEmbeddings {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let mut v = vec![0.0f32; 8];
                v[0] = (i + 1) as f32;
                v[1] = t.len() as f32;
                v
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_review_json_plain() {
        let text = r#"{
          "summary": "Looks fine.",
          "risk_score": 10,
          "comments": [
            {
              "file_path": "src/lib.rs",
              "line": 42,
              "severity": "warning",
              "message": "Unused variable",
              "suggestion": "Remove it."
            }
          ]
        }"#;
        let result = parse_review_json(text).unwrap();
        assert_eq!(result.risk_score, 10);
        assert_eq!(result.comments.len(), 1);
        assert_eq!(result.comments[0].line, Some(42));
    }

    #[test]
    fn parse_review_json_wrapped() {
        let text = "```json\n{\"summary\": \"s\", \"risk_score\": 0, \"comments\": []}\n```";
        let result = parse_review_json(text).unwrap();
        assert_eq!(result.comments.len(), 0);
    }

    #[test]
    fn truncate_works() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello…");
    }
}