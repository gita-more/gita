//! MCP Server for Gita.
//!
//! Exposes Gita's capabilities as MCP tools (Model Context Protocol).
//! This implementation speaks JSON-RPC 2.0 over stdio (for local tools
//! like Claude Desktop) and over HTTP (for remote clients).
//!
//! Implemented tools:
//! - list_repositories
//! - read_file
//! - list_commits
//! - search_code
//! - create_pull_request
//! - get_pull_request
//! - get_review
//! - create_issue
//! - list_events

use gita_db::Db;
use gita_git::GitManager;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use tracing::{debug, info};

/// Shared state for the MCP server.
#[derive(Clone)]
pub struct McpState {
    pub db: Db,
    pub git: Arc<GitManager>,
}

/// A tool definition exposed to MCP clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// The MCP server.
#[derive(Clone)]
pub struct McpServer {
    pub state: McpState,
    pub tools: Vec<Tool>,
}

impl McpServer {
    pub fn new(state: McpState) -> Self {
        let tools = vec![
            Tool {
                name: "list_repositories".to_string(),
                description: "List all repositories hosted on Gita.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "owner": {"type": "string", "description": "Filter by owner username (optional)"},
                        "limit": {"type": "integer", "description": "Max results (default 100)"}
                    }
                }),
            },
            Tool {
                name: "read_file".to_string(),
                description: "Read a file from a repository.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "required": ["owner", "repo", "path"],
                    "properties": {
                        "owner": {"type": "string"},
                        "repo": {"type": "string"},
                        "path": {"type": "string"},
                        "ref": {"type": "string", "description": "Branch or commit (default default_branch)"}
                    }
                }),
            },
            Tool {
                name: "list_commits".to_string(),
                description: "List recent commits of a repository.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "required": ["owner", "repo"],
                    "properties": {
                        "owner": {"type": "string"},
                        "repo": {"type": "string"},
                        "ref": {"type": "string"},
                        "limit": {"type": "integer", "default": 30}
                    }
                }),
            },
            Tool {
                name: "search_code".to_string(),
                description: "Semantic (keyword) search across repository code.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": {"type": "string"},
                        "repo": {"type": "string", "description": "owner/name to restrict search"},
                        "limit": {"type": "integer", "default": 20}
                    }
                }),
            },
            Tool {
                name: "create_pull_request".to_string(),
                description: "Create a pull request in a repository.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "required": ["owner", "repo", "title", "source_branch"],
                    "properties": {
                        "owner": {"type": "string"},
                        "repo": {"type": "string"},
                        "title": {"type": "string"},
                        "body": {"type": "string"},
                        "source_branch": {"type": "string"},
                        "target_branch": {"type": "string"}
                    }
                }),
            },
            Tool {
                name: "get_pull_request".to_string(),
                description: "Get a pull request by number, including its AI reviews.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "required": ["owner", "repo", "number"],
                    "properties": {
                        "owner": {"type": "string"},
                        "repo": {"type": "string"},
                        "number": {"type": "integer"}
                    }
                }),
            },
            Tool {
                name: "get_review".to_string(),
                description: "Get an AI review by id.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "required": ["review_id"],
                    "properties": {
                        "review_id": {"type": "integer"}
                    }
                }),
            },
            Tool {
                name: "create_issue".to_string(),
                description: "Create an issue in a repository.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "required": ["owner", "repo", "title"],
                    "properties": {
                        "owner": {"type": "string"},
                        "repo": {"type": "string"},
                        "title": {"type": "string"},
                        "body": {"type": "string"}
                    }
                }),
            },
            Tool {
                name: "list_events".to_string(),
                description: "List the AI-readable event stream (pushes, PRs, reviews, issues).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "repo": {"type": "string"},
                        "type": {"type": "string"},
                        "limit": {"type": "integer", "default": 50}
                    }
                }),
            },
        ];
        Self { state, tools }
    }

    /// Handle a JSON-RPC request and produce a response.
    pub async fn handle_request(&self, request: serde_json::Value) -> serde_json::Value {
        let id = request.get("id").cloned();
        let method = request
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let result = match method.as_str() {
            "initialize" => {
                info!("MCP initialize");
                serde_json::json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {
                        "tools": { "listChanged": false }
                    },
                    "serverInfo": {"name": "gita-mcp", "version": "0.0.1"}
                })
            }
            "tools/list" => {
                serde_json::json!({
                    "tools": self.tools
                })
            }
            "tools/call" => {
                let params = request.get("params").cloned().unwrap_or(serde_json::json!({}));
                let tool_name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));
                match self.call_tool(&tool_name, &arguments).await {
                    Ok(content) => serde_json::json!({
                        "content": [{"type": "text", "text": content}],
                        "isError": false,
                    }),
                    Err(e) => serde_json::json!({
                        "content": [{"type": "text", "text": format!("error: {e}")}],
                        "isError": true,
                    }),
                }
            }
            "resources/list" => serde_json::json!({ "resources": [] }),
            "ping" => serde_json::json!({}),
            _ => {
                serde_json::json!({
                    "error": {"code": -32601, "message": format!("method not found: {method}")}
                })
            }
        };

        let id = request.get("id").cloned();
        if id.is_none() {
            // Notification — no response.
            serde_json::Value::Null
        } else {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id.unwrap_or(serde_json::Value::Null),
                "result": result,
            })
        }
    }

    async fn call_tool(&self, name: &str, args: &serde_json::Value) -> Result<String, String> {
        match name {
            "list_repositories" => {
                let owner = args.get("owner").and_then(|v| v.as_str());
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
                let repos = self
                    .state
                    .db
                    .list_repositories(owner, limit)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(serde_json::to_string_pretty(&repos).unwrap_or_default())
            }
            "read_file" => {
                let owner = req_str(args, "owner")?;
                let repo = req_str(args, "repo")?;
                let path = req_str(args, "path")?;
                let repository = self
                    .state
                    .db
                    .get_repository_by_name(&owner, &repo)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("repo not found: {owner}/{repo}"))?;
                let reference = args
                    .get("ref")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&repository.default_branch);
                let file = self
                    .state
                    .git
                    .read_file(&owner, &repo, reference, &path)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("file not found: {path}"))?;
                Ok(serde_json::to_string_pretty(&file).unwrap_or_default())
            }
            "list_commits" => {
                let owner = req_str(args, "owner")?;
                let repo = req_str(args, "repo")?;
                let repository = self
                    .state
                    .db
                    .get_repository_by_name(&owner, &repo)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("repo not found: {owner}/{repo}"))?;
                let reference = args
                    .get("ref")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&repository.default_branch);
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(30) as usize;
                let commits = self
                    .state
                    .git
                    .list_commits(&owner, &repo, reference, limit)
                    .map_err(|e| e.to_string())?;
                Ok(serde_json::to_string_pretty(&commits).unwrap_or_default())
            }
            "search_code" => {
                let query = req_str(args, "query")?;
                let repo = args.get("repo").and_then(|v| v.as_str());
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
                let repo_id = if let Some(full) = repo {
                    let (owner, name) = full.split_once('/').ok_or("repo must be owner/name")?;
                    let r = self
                        .state
                        .db
                        .get_repository_by_name(owner, name)
                        .await
                        .map_err(|e| e.to_string())?;
                    r.map(|r| r.id)
                } else {
                    None
                };
                // Keyword search via the DB (FTS5).
                let hits = search_hits(&self.state.db, &query, repo_id, limit).await?;
                Ok(serde_json::to_string_pretty(&hits).unwrap_or_default())
            }
            "create_pull_request" => {
                let owner = req_str(args, "owner")?;
                let repo = req_str(args, "repo")?;
                let title = req_str(args, "title")?;
                let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("");
                let source_branch = req_str(args, "source_branch")?;
                let repository = self
                    .state
                    .db
                    .get_repository_by_name(&owner, &repo)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("repo not found: {owner}/{repo}"))?;
                let target_branch = args
                    .get("target_branch")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&repository.default_branch)
                    .to_string();
                let head_sha = self
                    .state
                    .git
                    .head_sha(&owner, &repo, &source_branch)
                    .map_err(|e| format!("bad source branch: {e}"))?;
                let base_sha = self
                    .state
                    .git
                    .head_sha(&owner, &repo, &target_branch)
                    .map_err(|e| format!("bad target branch: {e}"))?;
                let pr = self
                    .state
                    .db
                    .create_pull_request(
                        repository.id,
                        &title,
                        body,
                        &source_branch,
                        &target_branch,
                        0, // system / agent user
                        "mcp-agent",
                        &base_sha,
                        &head_sha,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(serde_json::to_string_pretty(&pr).unwrap_or_default())
            }
            "get_pull_request" => {
                let owner = req_str(args, "owner")?;
                let repo = req_str(args, "repo")?;
                let number = args
                    .get("number")
                    .and_then(|v| v.as_i64())
                    .ok_or("number required")?;
                let repository = self
                    .state
                    .db
                    .get_repository_by_name(&owner, &repo)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("repo not found: {owner}/{repo}"))?;
                let pr = self
                    .state
                    .db
                    .get_pull_request(repository.id, number)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("pr #{number} not found"))?;
                Ok(serde_json::to_string_pretty(&pr).unwrap_or_default())
            }
            "get_review" => {
                let review_id = args
                    .get("review_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("review_id required")?;
                let review = self
                    .state
                    .db
                    .get_review(review_id)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("review #{review_id} not found"))?;
                Ok(serde_json::to_string_pretty(&review).unwrap_or_default())
            }
            "create_issue" => {
                let owner = req_str(args, "owner")?;
                let repo = req_str(args, "repo")?;
                let title = req_str(args, "title")?;
                let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("");
                let repository = self
                    .state
                    .db
                    .get_repository_by_name(&owner, &repo)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("repo not found: {owner}/{repo}"))?;
                let issue = self
                    .state
                    .db
                    .create_issue(repository.id, &title, body, 0, "mcp-agent")
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(serde_json::to_string_pretty(&issue).unwrap_or_default())
            }
            "list_events" => {
                let repo = args.get("repo").and_then(|v| v.as_str());
                let event_type = args.get("type").and_then(|v| v.as_str());
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
                let repo_id = if let Some(full) = repo {
                    let (owner, name) = full.split_once('/').ok_or("repo must be owner/name")?;
                    self.state
                        .db
                        .get_repository_by_name(owner, name)
                        .await
                        .map_err(|e| e.to_string())?
                        .map(|r| r.id)
                } else {
                    None
                };
                let events = self
                    .state
                    .db
                    .list_events(repo_id, event_type, limit)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(serde_json::to_string_pretty(&events).unwrap_or_default())
            }
            _ => Err(format!("unknown tool: {name}")),
        }
    }
}

fn req_str(args: &serde_json::Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("missing required argument: {key}"))
}

/// Keyword search using the FTS5 table (used by the MCP search_code tool).
async fn search_hits(
    db: &Db,
    query: &str,
    repo_id: Option<i64>,
    limit: usize,
) -> Result<Vec<serde_json::Value>, String> {
    db.raw_query(
        "CREATE VIRTUAL TABLE IF NOT EXISTS code_search USING fts5(
            repo_id UNINDEXED,
            repo_name,
            file_path,
            content,
            tokenize = 'porter unicode61'
        )",
    )
    .await
    .map_err(|e| e.to_string())?;
    let query_escaped = query.replace('\'', "''");
    let mut sql = format!(
        "SELECT repo_id, repo_name, file_path, content, bm25(code_search) AS score
         FROM code_search WHERE code_search MATCH '{}'",
        query_escaped
    );
    if let Some(rid) = repo_id {
        sql.push_str(&format!(" AND repo_id = {}", rid));
    }
    sql.push_str(&format!(" ORDER BY score LIMIT {}", limit));
    let rows = sqlx::query(&sql)
        .fetch_all(db.pool())
        .await
        .map_err(|e| e.to_string())?;
    let mut hits = Vec::new();
    for row in rows {
        use sqlx::Row;
        hits.push(serde_json::json!({
            "repo_id": row.try_get::<i64,_>("repo_id").unwrap_or(0),
            "repo_name": row.try_get::<String,_>("repo_name").unwrap_or_default(),
            "file_path": row.try_get::<String,_>("file_path").unwrap_or_default(),
            "content": row.try_get::<String,_>("content").unwrap_or_default(),
            "score": row.try_get::<f64,_>("score").unwrap_or(0.0),
        }));
    }
    Ok(hits)
}

/// Run the MCP server over stdio: read JSON-RPC lines from stdin,
/// write responses to stdout.
pub async fn run_stdio(state: McpState) -> anyhow::Result<()> {
    let server = McpServer::new(state);
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let mut writer = stdout;
    while let Some(line) = reader.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                let err = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": format!("parse error: {e}")}
                });
                writer.write_all(serde_json::to_string(&err)?.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                continue;
            }
        };
        let response = server.handle_request(parsed).await;
        if response.is_null() {
            continue;
        }
        writer.write_all(serde_json::to_string(&response)?.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }
    Ok(())
}