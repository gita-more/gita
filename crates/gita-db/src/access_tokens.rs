//! Data access for Gita, split by domain.

use anyhow::Result;
use chrono::{DateTime, Utc};
use gita_core::*;
use sqlx::Row;

use crate::Db;
use crate::users::row_to_user;

pub fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

impl Db {
    /// Create a personal access token.
    ///
    /// Only the SHA-256 hash of the secret is stored; the caller is
    /// responsible for showing the plaintext to the user exactly once
    /// (the returned `AccessToken` carries it).
    pub async fn create_access_token(
        &self,
        user_id: i64,
        token: &str,
        name: &str,
    ) -> Result<AccessToken> {
        let created_at = Utc::now().to_rfc3339();
        let hash = hash_token(token);
        let row = sqlx::query(
            "INSERT INTO access_tokens (user_id, token, token_hash, name, created_at)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id",
        )
        .bind(user_id)
        // The plaintext column is kept empty for new tokens so the
        // secret is not recoverable from the database.
        .bind("")
        .bind(&hash)
        .bind(name)
        .bind(&created_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(AccessToken {
            id: row.try_get("id").unwrap_or(0),
            user_id,
            token: token.to_string(),
            name: name.to_string(),
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    }

    /// Resolve the user a personal access token belongs to.
    ///
    /// New tokens are matched by their SHA-256 hash. Tokens issued
    /// before the hashing migration still hold their plaintext, so they
    /// are matched as a fallback and then *upgraded* in place — the
    /// plaintext is replaced by the hash, so it does not linger in the
    /// database.
    pub async fn get_user_by_token(&self, token: &str) -> Result<Option<User>> {
        let hash = hash_token(token);
        let rows = sqlx::query(
            "SELECT u.id, u.username, u.email, u.password_hash, u.created_at, u.locale, u.is_admin,
                    t.id AS token_id, t.token_hash, t.token
             FROM access_tokens t JOIN users u ON u.id = t.user_id
             WHERE t.token_hash = $1 OR (t.token_hash IS NULL AND t.token = $2)
             LIMIT 1",
        )
        .bind(&hash)
        .bind(token)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        let row = &rows[0];
        // A legacy row (matched by plaintext) is upgraded to the hashed
        // form so the secret stops being stored.
        if row.try_get::<Option<String>, _>("token_hash")?.is_none() {
            if let Ok(token_id) = row.try_get::<i64, _>("token_id") {
                let _ = sqlx::query(
                    "UPDATE access_tokens SET token = '', token_hash = $1 WHERE id = $2",
                )
                .bind(&hash)
                .bind(token_id)
                .execute(&self.pool)
                .await;
            }
        }
        Ok(Some(row_to_user(row)?))
    }

    /// List a user's personal access tokens.
    ///
    /// The `token` field of each returned row is **empty**: the secret
    /// is only ever shown once, when the token is created. The API
    /// layer turns that into a short prefix so the user can tell the
    /// tokens apart.
    pub async fn list_tokens(&self, user_id: i64) -> Result<Vec<AccessToken>> {
        let rows = sqlx::query(
            "SELECT id, user_id, name, created_at FROM access_tokens WHERE user_id = $1 ORDER BY id DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let mut tokens = Vec::new();
        for row in rows {
            let created_at: String = row.try_get("created_at")?;
            tokens.push(AccessToken {
                id: row.try_get("id")?,
                user_id: row.try_get("user_id")?,
                // Never re-expose the secret.
                token: String::new(),
                name: row.try_get("name")?,
                created_at: DateTime::parse_from_rfc3339(&created_at)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            });
        }
        Ok(tokens)
    }

    pub async fn delete_token(&self, id: i64, user_id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM access_tokens WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}
