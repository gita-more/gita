//! Data access for Gita, split by domain.

use anyhow::Result;
use chrono::Utc;
use sqlx::Row;

use crate::Db;

#[derive(Debug, Clone)]
pub struct WebhookRow {
    pub id: i64,
    pub repo_id: i64,
    pub url: String,
    pub secret: String,
    pub events: Vec<String>,
    pub active: bool,
    pub created_at: String,
}

impl Db {
    pub async fn create_webhook(
        &self,
        repo_id: i64,
        url: &str,
        events: &[String],
        secret: &str,
        active: bool,
    ) -> Result<WebhookRow> {
        let created_at = Utc::now().to_rfc3339();
        let events_csv = events.join(",");
        let row = sqlx::query(
            "INSERT INTO webhooks (repo_id, url, secret, events, active, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id",
        )
        .bind(repo_id)
        .bind(url)
        .bind(secret)
        .bind(&events_csv)
        .bind(active)
        .bind(&created_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(WebhookRow {
            id: row.try_get("id").unwrap_or(0),
            repo_id,
            url: url.to_string(),
            secret: secret.to_string(),
            events: events.to_vec(),
            active,
            created_at,
        })
    }

    pub async fn list_webhooks(&self, repo_id: i64) -> Result<Vec<WebhookRow>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, url, secret, events, active, created_at FROM webhooks
             WHERE repo_id = $1 ORDER BY id DESC",
        )
        .bind(repo_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let events_csv: String = row.try_get("events")?;
            let events: Vec<String> = if events_csv.is_empty() {
                Vec::new()
            } else {
                events_csv.split(',').map(|s| s.to_string()).collect()
            };
            out.push(WebhookRow {
                id: row.try_get("id")?,
                repo_id: row.try_get("repo_id")?,
                url: row.try_get("url")?,
                secret: row.try_get("secret")?,
                events,
                active: row.try_get("active")?,
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(out)
    }

    pub async fn delete_webhook(&self, id: i64, repo_id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM webhooks WHERE id = $1 AND repo_id = $2")
            .bind(id)
            .bind(repo_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}
