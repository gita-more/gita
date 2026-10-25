//! Data access for Gita, split by domain.

use anyhow::Result;
use chrono::Utc;
use sqlx::Row;

use crate::Db;

#[derive(Debug, Clone)]
pub struct PagesRow {
    pub repo_id: i64,
    pub enabled: bool,
    pub branch: String,
    pub path: String,
    pub created_at: String,
    pub updated_at: String,
}

impl Db {
    /// Read the Pages configuration for a repository, if any.
    pub async fn get_pages(&self, repo_id: i64) -> Result<Option<PagesRow>> {
        let rows = sqlx::query(
            "SELECT repo_id, enabled, branch, path, created_at, updated_at FROM repo_pages
             WHERE repo_id = $1",
        )
        .bind(repo_id)
        .fetch_all(&self.pool)
        .await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(PagesRow {
            repo_id: row.try_get("repo_id")?,
            enabled: row.try_get("enabled")?,
            branch: row.try_get("branch")?,
            path: row.try_get("path")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        }))
    }

    /// Insert or update the Pages configuration for a repository. A repository
    /// has at most one Pages site, so the second PUT updates the existing row
    /// rather than creating a duplicate.
    pub async fn upsert_pages(
        &self,
        repo_id: i64,
        enabled: bool,
        branch: &str,
        path: &str,
    ) -> Result<PagesRow> {
        let now = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO repo_pages (repo_id, enabled, branch, path, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $5)
             ON CONFLICT (repo_id) DO UPDATE
               SET enabled = EXCLUDED.enabled,
                   branch  = EXCLUDED.branch,
                   path    = EXCLUDED.path,
                   updated_at = EXCLUDED.updated_at
             RETURNING repo_id, enabled, branch, path, created_at, updated_at",
        )
        .bind(repo_id)
        .bind(enabled)
        .bind(branch)
        .bind(path)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        Ok(PagesRow {
            repo_id: row.try_get("repo_id")?,
            enabled: row.try_get("enabled")?,
            branch: row.try_get("branch")?,
            path: row.try_get("path")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }

    /// Remove the Pages configuration for a repository. Returns true when a
    /// row was actually deleted.
    pub async fn delete_pages(&self, repo_id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM repo_pages WHERE repo_id = $1")
            .bind(repo_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}
