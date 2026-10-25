//! Data access for Gita, split by domain.

use anyhow::Result;
use chrono::{DateTime, Utc};
use gita_core::*;
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::Db;

impl Db {
    /// Create a repository.
    ///
    /// `group_id` is `None` for a personal repository and `Some(..)` for
    /// one in a group namespace. The group is written as part of the
    /// insert rather than patched afterwards, so the row is never
    /// observable without it.
    pub async fn create_repository(
        &self,
        owner_id: i64,
        owner_name: &str,
        name: &str,
        description: &str,
        is_private: bool,
        default_branch: &str,
        group_id: Option<i64>,
    ) -> Result<Repository> {
        let created_at = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO repositories (owner_id, owner_name, name, description, is_private, default_branch, created_at, group_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id",
        )
        .bind(owner_id)
        .bind(owner_name)
        .bind(name)
        .bind(description)
        .bind(is_private as i64)
        .bind(default_branch)
        .bind(&created_at)
        .bind(group_id)
        .fetch_one(&self.pool)
        .await?;
        let id: i64 = row.try_get("id").unwrap_or(0);
        Ok(Repository {
            id,
            owner_id,
            owner_name: owner_name.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            is_private,
            default_branch: default_branch.to_string(),
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            group_id,
        })
    }

    pub async fn get_repository(&self, id: i64) -> Result<Option<Repository>> {
        let rows = sqlx::query(
            "SELECT id, owner_id, owner_name, name, description, is_private, default_branch, created_at, group_id
             FROM repositories WHERE id = $1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_repo(&rows[0])?))
    }

    pub async fn get_repository_by_name(
        &self,
        owner_name: &str,
        name: &str,
    ) -> Result<Option<Repository>> {
        let rows = sqlx::query(
            "SELECT id, owner_id, owner_name, name, description, is_private, default_branch, created_at, group_id
             FROM repositories WHERE owner_name = $1 AND name = $2",
        )
        .bind(owner_name)
        .bind(name)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_repo(&rows[0])?))
    }

    pub async fn list_repositories(
        &self,
        owner_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Repository>> {
        let rows = if let Some(owner) = owner_name {
            sqlx::query(
                "SELECT id, owner_id, owner_name, name, description, is_private, default_branch, created_at, group_id
                 FROM repositories WHERE owner_name = $2 ORDER BY id DESC LIMIT $1",
            )
            .bind(limit as i64)
            .bind(owner)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT id, owner_id, owner_name, name, description, is_private, default_branch, created_at, group_id
                 FROM repositories ORDER BY id DESC LIMIT $1",
            )
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?
        };
        rows.iter().map(row_to_repo).collect()
    }

    pub async fn delete_repository(&self, id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM repositories WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Search public repositories by name or description (ILIKE). Hidden
    /// (`is_private = true`) rows are excluded unless the search query is
    /// empty — callers that need owner-scoped lookups should use the
    /// `?owner=` filter on the listing endpoint instead.
    pub async fn search_repositories(&self, query: &str, limit: usize) -> Result<Vec<Repository>> {
        let pattern = format!("%{query}%");
        let rows = sqlx::query(
            "SELECT id, owner_id, owner_name, name, description, is_private, default_branch, created_at, group_id
             FROM repositories
             WHERE (name ILIKE $1 OR description ILIKE $1)
               AND is_private = 0
             ORDER BY id DESC
             LIMIT $2",
        )
        .bind(pattern)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_repo).collect()
    }

    pub async fn update_repository_description(&self, id: i64, description: &str) -> Result<()> {
        sqlx::query("UPDATE repositories SET description = $2 WHERE id = $1")
            .bind(id)
            .bind(description)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Update a repository's visibility (`is_private`).
    pub async fn update_repository_visibility(&self, id: i64, is_private: bool) -> Result<()> {
        sqlx::query("UPDATE repositories SET is_private = $2 WHERE id = $1")
            .bind(id)
            .bind(is_private as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Rename a repository. Returns the updated row.
    pub async fn rename_repository(
        &self,
        id: i64,
        new_owner_name: &str,
        new_name: &str,
    ) -> Result<Option<Repository>> {
        let rows = sqlx::query(
            "UPDATE repositories SET owner_name = $2, name = $3 WHERE id = $1
             RETURNING id, owner_id, owner_name, name, description, is_private, default_branch, created_at, group_id",
        )
        .bind(id)
        .bind(new_owner_name)
        .bind(new_name)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.first().map(row_to_repo).transpose()?)
    }

    /// Transfer a repository to a new owner (user or group).
    pub async fn transfer_repository(
        &self,
        id: i64,
        new_owner_id: i64,
        new_owner_name: &str,
        group_id: Option<i64>,
    ) -> Result<Option<Repository>> {
        let rows = sqlx::query(
            "UPDATE repositories SET owner_id = $2, owner_name = $3, group_id = $4 WHERE id = $1
             RETURNING id, owner_id, owner_name, name, description, is_private, default_branch, created_at, group_id",
        )
        .bind(id)
        .bind(new_owner_id)
        .bind(new_owner_name)
        .bind(group_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.first().map(row_to_repo).transpose()?)
    }
}

pub(crate) fn row_to_repo(row: &PgRow) -> Result<Repository> {
    let created_at: String = row.try_get("created_at")?;
    Ok(Repository {
        id: row.try_get("id")?,
        owner_id: row.try_get("owner_id")?,
        owner_name: row.try_get("owner_name")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        is_private: row.try_get::<i64, _>("is_private")? != 0,
        default_branch: row.try_get("default_branch")?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        group_id: row.try_get::<Option<i64>, _>("group_id")?,
    })
}
