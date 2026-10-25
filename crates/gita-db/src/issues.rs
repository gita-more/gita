//! Data access for Gita, split by domain.

use anyhow::Result;
use chrono::{DateTime, Utc};
use gita_core::*;
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::Db;

impl Db {
    pub async fn create_issue(
        &self,
        repo_id: i64,
        title: &str,
        body: &str,
        author_id: i64,
        author_name: &str,
    ) -> Result<Issue> {
        let now = Utc::now().to_rfc3339();
        let rows = sqlx::query(
            "SELECT COALESCE(MAX(number), 0) + 1 AS next_num FROM issues WHERE repo_id = $1",
        )
        .bind(repo_id)
        .fetch_all(&self.pool)
        .await?;
        let number: i64 = rows
            .first()
            .map(|r| r.try_get::<i64, _>("next_num").unwrap_or(1))
            .unwrap_or(1);
        let row = sqlx::query(
            "INSERT INTO issues (repo_id, number, title, body, status, author_id, author_name, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'open', $5, $6, $7, $8)
             RETURNING id",
        )
        .bind(repo_id)
        .bind(number)
        .bind(title)
        .bind(body)
        .bind(author_id)
        .bind(author_name)
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        let id: i64 = row.try_get("id").unwrap_or(0);
        Ok(Issue {
            id,
            repo_id,
            number,
            title: title.to_string(),
            body: body.to_string(),
            status: IssueStatus::Open,
            author_id,
            author_name: author_name.to_string(),
            created_at: DateTime::parse_from_rfc3339(&now)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            updated_at: DateTime::parse_from_rfc3339(&now)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            closed_at: None,
        })
    }

    pub async fn get_issue(&self, repo_id: i64, number: i64) -> Result<Option<Issue>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, number, title, body, status, author_id, author_name, created_at, updated_at, closed_at
             FROM issues WHERE repo_id = $1 AND number = $2",
        )
        .bind(repo_id)
        .bind(number)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_issue(&rows[0])?))
    }

    pub async fn list_issues(
        &self,
        repo_id: i64,
        status: Option<IssueStatus>,
        limit: usize,
    ) -> Result<Vec<Issue>> {
        let rows = if let Some(st) = status {
            sqlx::query(
                "SELECT id, repo_id, number, title, body, status, author_id, author_name, created_at, updated_at, closed_at
                 FROM issues WHERE repo_id = $1 AND status = $3 ORDER BY id DESC LIMIT $2",
            )
            .bind(repo_id)
            .bind(limit as i64)
            .bind(st.to_string())
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT id, repo_id, number, title, body, status, author_id, author_name, created_at, updated_at, closed_at
                 FROM issues WHERE repo_id = $1 ORDER BY id DESC LIMIT $2",
            )
            .bind(repo_id)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?
        };
        rows.iter().map(row_to_issue).collect()
    }

    pub async fn update_issue_status(&self, id: i64, status: IssueStatus) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let closed_at = if status == IssueStatus::Closed {
            Some(now.clone())
        } else {
            None
        };
        sqlx::query("UPDATE issues SET status = $2, updated_at = $3, closed_at = $4 WHERE id = $1")
            .bind(id)
            .bind(status.to_string())
            .bind(&now)
            .bind(closed_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn row_to_issue(row: &PgRow) -> Result<Issue> {
    let status: String = row.try_get("status")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;
    let closed_at: Option<String> = row.try_get("closed_at")?;
    Ok(Issue {
        id: row.try_get("id")?,
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
