//! Data access for Gita, split by domain.

use anyhow::Result;
use chrono::{DateTime, Utc};
use gita_core::*;
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::Db;

impl Db {
    /// Add a comment to a pull request. `author_id`/`author_name` are the
    /// authenticated user; `repo_id` is denormalized for cascade deletes.
    pub async fn create_pr_comment(
        &self,
        repo_id: i64,
        pr_id: i64,
        author_id: i64,
        author_name: &str,
        body: &str,
    ) -> Result<Comment> {
        let now = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO comments (repo_id, pr_id, issue_id, author_id, author_name, body, created_at, updated_at)
             VALUES ($1, $2, NULL, $3, $4, $5, $6, $6)
             RETURNING id, repo_id, pr_id, issue_id, author_id, author_name, body, created_at, updated_at",
        )
        .bind(repo_id)
        .bind(pr_id)
        .bind(author_id)
        .bind(author_name)
        .bind(body)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row_to_comment(&row)?)
    }

    /// Add a comment to an issue.
    pub async fn create_issue_comment(
        &self,
        repo_id: i64,
        issue_id: i64,
        author_id: i64,
        author_name: &str,
        body: &str,
    ) -> Result<Comment> {
        let now = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO comments (repo_id, pr_id, issue_id, author_id, author_name, body, created_at, updated_at)
             VALUES ($1, NULL, $2, $3, $4, $5, $6, $6)
             RETURNING id, repo_id, pr_id, issue_id, author_id, author_name, body, created_at, updated_at",
        )
        .bind(repo_id)
        .bind(issue_id)
        .bind(author_id)
        .bind(author_name)
        .bind(body)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row_to_comment(&row)?)
    }

    /// List comments for a pull request, oldest first.
    pub async fn list_pr_comments(&self, pr_id: i64, limit: usize) -> Result<Vec<Comment>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, pr_id, issue_id, author_id, author_name, body, created_at, updated_at
             FROM comments WHERE pr_id = $1 ORDER BY id ASC LIMIT $2",
        )
        .bind(pr_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_comment).collect()
    }

    /// List comments for an issue, oldest first.
    pub async fn list_issue_comments(&self, issue_id: i64, limit: usize) -> Result<Vec<Comment>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, pr_id, issue_id, author_id, author_name, body, created_at, updated_at
             FROM comments WHERE issue_id = $1 ORDER BY id ASC LIMIT $2",
        )
        .bind(issue_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_comment).collect()
    }

    /// Get a single comment by id.
    pub async fn get_comment(&self, id: i64) -> Result<Option<Comment>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, pr_id, issue_id, author_id, author_name, body, created_at, updated_at
             FROM comments WHERE id = $1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.first().map(row_to_comment).transpose()?)
    }

    /// Delete a comment. Returns whether a row was deleted.
    pub async fn delete_comment(&self, id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM comments WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}

fn row_to_comment(row: &PgRow) -> Result<Comment> {
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;
    Ok(Comment {
        id: row.try_get("id")?,
        repo_id: row.try_get("repo_id")?,
        pr_id: row.try_get("pr_id")?,
        issue_id: row.try_get("issue_id")?,
        author_id: row.try_get("author_id")?,
        author_name: row.try_get("author_name")?,
        body: row.try_get("body")?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    })
}
