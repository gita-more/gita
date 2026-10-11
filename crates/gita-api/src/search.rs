//! Semantic code search.
//!
//! This module provides a search backend abstraction. The default
//! backend uses SQLite FTS5 (available in the `Any` driver when SQLite
//! is the underlying database), which requires zero external services.
//!
//! A real embedding-based backend can be plugged in later by
//! implementing [`SearchBackend`].

use crate::error::{ApiError, ApiResult};
use gita_db::Db;
use gita_git::GitManager;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A search hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub repo_id: i64,
    pub repo_name: String,
    pub file_path: String,
    pub content: String,
    pub score: f32,
}

/// The search backend trait.
#[async_trait::async_trait]
pub trait SearchBackend: Send + Sync {
    /// Index a file's content for a repository.
    async fn index_file(
        &self,
        repo_id: i64,
        repo_name: &str,
        file_path: &str,
        content: &str,
    ) -> Result<(), String>;

    /// Search for `query` and return hits.
    async fn search(&self, query: &str, repo_id: Option<i64>, limit: usize)
        -> Result<Vec<SearchHit>, String>;

    /// Remove all indexes for a repository.
    async fn clear_repo(&self, repo_id: i64) -> Result<(), String>;
}

/// The default FTS5-based search backend (works with SQLite).
#[derive(Debug, Clone)]
pub struct Fts5Backend {
    pub db: Db,
    pub git: Arc<GitManager>,
}

#[async_trait::async_trait]
impl SearchBackend for Fts5Backend {
    async fn index_file(
        &self,
        repo_id: i64,
        repo_name: &str,
        file_path: &str,
        content: &str,
    ) -> Result<(), String> {
        // Create the FTS5 table if needed (idempotent).
        self.db
            .raw_query(
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

        // Delete any existing entries for this file, then insert fresh.
        self.db
            .execute("DELETE FROM code_search WHERE repo_id = ? AND file_path = ?")
            .await
            .map_err(|e| e.to_string())?;

        // We need to bind parameters, so use a direct query.
        let pool = self.db.pool();
        let _ = pool;
        // For simplicity, escape single quotes in content.
        let content_escaped = content.replace('\'', "''");
        let path_escaped = file_path.replace('\'', "''");
        let repo_escaped = repo_name.replace('\'', "''");
        let sql = format!(
            "INSERT INTO code_search (repo_id, repo_name, file_path, content) VALUES ({}, '{}', '{}', '{}')",
            repo_id,
            repo_escaped,
            path_escaped,
            content_escaped,
        );
        self.db.execute(&sql).await.map_err(|e| e.to_string())
    }

    async fn search(
        &self,
        query: &str,
        repo_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<SearchHit>, String> {
        self.db
            .raw_query(
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
        let pool = self.db.pool();
        let rows = sqlx::query(&sql)
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;
        let mut hits = Vec::new();
        for row in rows {
            use sqlx::Row;
            let repo_id: i64 = row.try_get("repo_id").unwrap_or(0);
            let repo_name: String = row.try_get("repo_name").unwrap_or_default();
            let file_path: String = row.try_get("file_path").unwrap_or_default();
            let content: String = row.try_get("content").unwrap_or_default();
            let score: f64 = row.try_get("score").unwrap_or(0.0);
            hits.push(SearchHit {
                repo_id,
                repo_name: repo_name.clone(),
                file_path,
                content: truncate(&content, 500),
                score: score as f32,
            });
        }
        Ok(hits)
    }

    async fn clear_repo(&self, repo_id: i64) -> Result<(), String> {
        let sql = format!("DELETE FROM code_search WHERE repo_id = {}", repo_id);
        self.db.execute(&sql).await.map_err(|e| e.to_string())
    }
}

/// The search service facade used by the API.
#[derive(Clone)]
pub struct SearchService {
    pub backend: Arc<dyn SearchBackend>,
}

impl SearchService {
    pub fn new(backend: Arc<dyn SearchBackend>) -> Self {
        Self { backend }
    }

    pub async fn index_repository(
        &self,
        repo_id: i64,
        repo_name: &str,
        git: &GitManager,
        reference: &str,
    ) -> Result<(), ApiError> {
        // Walk the tree and index all text files.
        let (owner, name) = repo_name
            .split_once('/')
            .map(|(o, n)| (o.to_string(), n.to_string()))
            .ok_or_else(|| ApiError::Internal("invalid repo name".into()))?;
        let tree = git
            .list_tree(&owner, &name, reference, "")
            .map_err(ApiError::from)?;
        for entry in tree {
            if entry.kind == "blob" {
                if let Some(file) = git
                    .read_file(&owner, &name, reference, &entry.path)
                    .map_err(ApiError::from)?
                {
                    // Only index text files (skip binaries) — just index everything under a size cap.
                    if file.content.len() < 512 * 1024 {
                        self.backend
                            .index_file(repo_id, repo_name, &file.path, &file.content)
                            .await
                            .map_err(ApiError::Internal)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn search(
        &self,
        query: &str,
        repo_id: Option<i64>,
        limit: usize,
    ) -> ApiResult<Vec<SearchHit>> {
        self.backend
            .search(query, repo_id, limit)
            .await
            .map_err(ApiError::Internal)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}