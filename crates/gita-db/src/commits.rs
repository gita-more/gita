//! Data access for Gita, split by domain.

use anyhow::Result;
use chrono::{DateTime, Utc};
use gita_core::*;
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::Db;

impl Db {
    pub async fn insert_commit(&self, commit: &Commit) -> Result<i64> {
        sqlx::query(
            "INSERT INTO commits (repo_id, sha, author_name, author_email, message, parent_shas, authored_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (repo_id, sha) DO NOTHING",
        )
        .bind(commit.repo_id)
        .bind(&commit.sha)
        .bind(&commit.author_name)
        .bind(&commit.author_email)
        .bind(&commit.message)
        .bind(serde_json::to_string(&commit.parent_shas)?)
        .bind(commit.authored_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(0) // ON CONFLICT DO NOTHING does not return id; callers that need id
              // should re-query via get_commit. insert_commit is fire-and-forget
              // for the changeset-walk path.
    }

    pub async fn get_commit(&self, repo_id: i64, sha: &str) -> Result<Option<Commit>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, sha, author_name, author_email, message, parent_shas, authored_at
             FROM commits WHERE repo_id = $1 AND sha = $2",
        )
        .bind(repo_id)
        .bind(sha)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_commit(&rows[0])?))
    }

    pub async fn list_commits(&self, repo_id: i64, limit: usize) -> Result<Vec<Commit>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, sha, author_name, author_email, message, parent_shas, authored_at
             FROM commits WHERE repo_id = $1 ORDER BY authored_at DESC LIMIT $2",
        )
        .bind(repo_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_commit).collect()
    }

    pub async fn latest_commit_sha(&self, repo_id: i64) -> Result<Option<String>> {
        let rows = sqlx::query(
            "SELECT sha FROM commits WHERE repo_id = $1 ORDER BY authored_at DESC LIMIT 1",
        )
        .bind(repo_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.first().and_then(|r| r.try_get("sha").ok()))
    }
}

fn row_to_commit(row: &PgRow) -> Result<Commit> {
    let parent_shas: String = row.try_get("parent_shas")?;
    let authored_at: String = row.try_get("authored_at")?;
    Ok(Commit {
        id: row.try_get("id")?,
        repo_id: row.try_get("repo_id")?,
        sha: row.try_get("sha")?,
        author_name: row.try_get("author_name")?,
        author_email: row.try_get("author_email")?,
        message: row.try_get("message")?,
        parent_shas: serde_json::from_str(&parent_shas).unwrap_or_default(),
        authored_at: DateTime::parse_from_rfc3339(&authored_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    })
}
