//! Data access for Gita, split by domain.

use anyhow::Result;
use chrono::Utc;
use sqlx::Row;

use crate::Db;

#[derive(Debug, Clone)]
pub struct SshKeyRow {
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    pub public_key: String,
    pub created_at: String,
}

impl Db {
    pub async fn add_ssh_key(
        &self,
        user_id: i64,
        title: &str,
        public_key: &str,
    ) -> Result<SshKeyRow> {
        let created_at = Utc::now().to_rfc3339();
        // The (user_id, public_key) unique index makes a duplicate
        // registration a conflict rather than a second row. The same key
        // cannot be added twice, so the error is surfaced to the caller
        // instead of being swallowed.
        let row = sqlx::query(
            "INSERT INTO ssh_keys (user_id, title, public_key, created_at)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (user_id, public_key) DO UPDATE SET title = EXCLUDED.title
             RETURNING id",
        )
        .bind(user_id)
        .bind(title)
        .bind(public_key)
        .bind(&created_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(SshKeyRow {
            id: row.try_get("id").unwrap_or(0),
            user_id,
            title: title.to_string(),
            public_key: public_key.to_string(),
            created_at,
        })
    }

    pub async fn list_ssh_keys(&self, user_id: i64) -> Result<Vec<SshKeyRow>> {
        let rows = sqlx::query(
            "SELECT id, user_id, title, public_key, created_at FROM ssh_keys
             WHERE user_id = $1 ORDER BY id DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(SshKeyRow {
                id: row.try_get("id")?,
                user_id: row.try_get("user_id")?,
                title: row.try_get("title")?,
                public_key: row.try_get("public_key")?,
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(out)
    }

    pub async fn list_all_ssh_keys(&self) -> Result<Vec<SshKeyRow>> {
        let rows = sqlx::query("SELECT id, user_id, title, public_key, created_at FROM ssh_keys")
            .fetch_all(&self.pool)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(SshKeyRow {
                id: row.try_get("id")?,
                user_id: row.try_get("user_id")?,
                title: row.try_get("title")?,
                public_key: row.try_get("public_key")?,
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(out)
    }

    pub async fn delete_ssh_key(&self, id: i64, user_id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM ssh_keys WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn find_ssh_key_by_fingerprint(
        &self,
        fingerprint: &str,
    ) -> Result<Option<SshKeyRow>> {
        // Lookup a key by its SHA256 fingerprint (the form used by ssh-keygen
        // and OpenSSH starting from release 6.8). We compare against the
        // stored `public_key`'s decoded SHA256 fingerprint here; the SSH
        // server computes the fingerprint of the offered key and calls this.
        let all = self.list_all_ssh_keys().await?;
        for key in all {
            if ssh_fingerprint(&key.public_key).as_deref() == Some(fingerprint) {
                return Ok(Some(key));
            }
        }
        Ok(None)
    }
}

pub fn ssh_fingerprint(public_key: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    let trimmed = public_key.trim();
    let mut parts = trimmed.split_whitespace();
    let _alg = parts.next()?;
    let blob = parts.next()?;
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(blob)
        .ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    // base64(SHA256) without padding — matches OpenSSH output.
    let b64 = base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest);
    Some(format!("SHA256:{b64}"))
}

/// Returns `true` when `public_key` looks like a valid OpenSSH public key
/// line (algorithm + base64 blob), used by the API to reject junk before
/// it reaches the `ssh_keys` table.
pub fn ssh_key_is_valid(public_key: &str) -> bool {
    ssh_fingerprint(public_key).is_some()
}
