//! Data access for Gita, split by domain.

use anyhow::Result;
use chrono::{DateTime, Utc};
use gita_core::*;
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::Db;
use crate::repositories::row_to_repo;

impl Db {
    /// Create a new group owned by `owner_id`.
    pub async fn create_group(
        &self,
        name: &str,
        description: &str,
        owner_id: i64,
    ) -> Result<Group> {
        let created_at = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO groups (name, description, owner_id, created_at)
             VALUES ($1, $2, $3, $4)
             RETURNING id",
        )
        .bind(name)
        .bind(description)
        .bind(owner_id)
        .bind(&created_at)
        .fetch_one(&self.pool)
        .await?;
        let id: i64 = row.try_get("id").unwrap_or(0);

        // Owner is automatically a member with role 'owner'.
        sqlx::query(
            "INSERT INTO group_members (group_id, user_id, role)
             VALUES ($1, $2, 'owner')
             ON CONFLICT (group_id, user_id) DO NOTHING",
        )
        .bind(id)
        .bind(owner_id)
        .execute(&self.pool)
        .await?;

        Ok(Group {
            id,
            name: name.to_string(),
            description: description.to_string(),
            owner_id,
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    }

    /// Fetch a group by its unique name.
    pub async fn get_group_by_name(&self, name: &str) -> Result<Option<Group>> {
        let rows = sqlx::query(
            "SELECT id, name, description, owner_id, created_at FROM groups WHERE name = $1",
        )
        .bind(name)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.first().map(row_to_group).transpose()?)
    }

    /// List all groups the given user is a member of (including owned).
    pub async fn list_groups_for_user(&self, user_id: i64) -> Result<Vec<Group>> {
        let rows = sqlx::query(
            "SELECT g.id, g.name, g.description, g.owner_id, g.created_at
             FROM groups g
             JOIN group_members gm ON gm.group_id = g.id
             WHERE gm.user_id = $1
             ORDER BY g.name",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_group).collect()
    }

    /// List repositories that belong to a group.
    pub async fn list_group_repositories(&self, group_id: i64) -> Result<Vec<Repository>> {
        let rows = sqlx::query(
            "SELECT id, owner_id, owner_name, name, description, is_private, default_branch, created_at, group_id
             FROM repositories WHERE group_id = $1 ORDER BY name",
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_repo).collect()
    }

    /// Delete a group row. `group_members` rows cascade; repositories that
    /// belong to the group must be deleted (or reassigned) by the caller
    /// first — there is no `ON DELETE CASCADE` on `repositories.group_id`.
    pub async fn delete_group(&self, group_id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM groups WHERE id = $1")
            .bind(group_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// List all issues across all (public) repositories, joined with the
    /// repository's owner/name for display.
    pub async fn list_all_issues(
        &self,
        status: Option<IssueStatus>,
        limit: usize,
    ) -> Result<Vec<(Issue, Repository)>> {
        let rows = if let Some(st) = status {
            sqlx::query(
                "SELECT i.id AS issue_id, i.repo_id, i.number, i.title, i.body, i.status,
                        i.author_id, i.author_name, i.created_at AS issue_created_at,
                        i.updated_at AS issue_updated_at, i.closed_at,
                        r.id AS repo_id, r.owner_id, r.owner_name, r.name, r.description,
                        r.is_private, r.default_branch, r.created_at AS repo_created_at, r.group_id
                 FROM issues i
                 JOIN repositories r ON r.id = i.repo_id
                 WHERE i.status = $2
                 ORDER BY i.id DESC LIMIT $1",
            )
            .bind(limit as i64)
            .bind(st.to_string())
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT i.id AS issue_id, i.repo_id, i.number, i.title, i.body, i.status,
                        i.author_id, i.author_name, i.created_at AS issue_created_at,
                        i.updated_at AS issue_updated_at, i.closed_at,
                        r.id AS repo_id, r.owner_id, r.owner_name, r.name, r.description,
                        r.is_private, r.default_branch, r.created_at AS repo_created_at, r.group_id
                 FROM issues i
                 JOIN repositories r ON r.id = i.repo_id
                 ORDER BY i.id DESC LIMIT $1",
            )
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?
        };

        rows.iter()
            .map(|row| {
                let issue = row_to_issue_alias(row)?;
                let repo = row_to_repo_alias(row)?;
                Ok((issue, repo))
            })
            .collect()
    }
}

fn row_to_issue_alias(row: &PgRow) -> Result<Issue> {
    let status: String = row.try_get("status")?;
    let created_at: String = row.try_get("issue_created_at")?;
    let updated_at: String = row.try_get("issue_updated_at")?;
    let closed_at: Option<String> = row.try_get("closed_at")?;
    Ok(Issue {
        id: row.try_get("issue_id")?,
        repo_id: row.try_get("repo_id")?,
        number: row.try_get("number")?,
        title: row.try_get("title")?,
        body: row.try_get("body")?,
        status: if status == "closed" {
            IssueStatus::Closed
        } else {
            IssueStatus::Open
        },
        author_id: row.try_get("author_id")?,
        author_name: row.try_get("author_name")?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        closed_at: closed_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc)),
    })
}

fn row_to_repo_alias(row: &PgRow) -> Result<Repository> {
    let created_at: String = row.try_get("repo_created_at")?;
    Ok(Repository {
        id: row.try_get("repo_id")?,
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

pub(crate) fn row_to_group(row: &PgRow) -> Result<Group> {
    let created_at: String = row.try_get("created_at")?;
    Ok(Group {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        owner_id: row.try_get("owner_id")?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    })
}
