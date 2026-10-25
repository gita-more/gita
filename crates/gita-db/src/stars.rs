//! Data access for Gita, split by domain.

use anyhow::Result;
use chrono::Utc;
use sqlx::Row;

use crate::Db;

impl Db {
    /// Add (or no-op) a star from `user_id` to `repo_id`. Returns true on
    /// insert, false when the row already existed.
    pub async fn add_star(&self, user_id: i64, repo_id: i64) -> Result<bool> {
        let created_at = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO stars (user_id, repo_id, created_at) VALUES ($1, $2, $3)
             ON CONFLICT (user_id, repo_id) DO NOTHING
             RETURNING user_id",
        )
        .bind(user_id)
        .bind(repo_id)
        .bind(&created_at)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    /// Remove a star. Returns whether a row was deleted.
    pub async fn remove_star(&self, user_id: i64, repo_id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM stars WHERE user_id = $1 AND repo_id = $2")
            .bind(user_id)
            .bind(repo_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Whether `user_id` has starred `repo_id`.
    pub async fn has_star(&self, user_id: i64, repo_id: i64) -> Result<bool> {
        let rows = sqlx::query("SELECT 1 AS ok FROM stars WHERE user_id = $1 AND repo_id = $2")
            .bind(user_id)
            .bind(repo_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(!rows.is_empty())
    }

    /// Total star count for a repository.
    pub async fn count_stars(&self, repo_id: i64) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*)::bigint AS n FROM stars WHERE repo_id = $1")
            .bind(repo_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("n").unwrap_or(0))
    }
}
