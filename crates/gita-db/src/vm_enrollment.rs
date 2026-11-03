//! Node enrollment tokens and per-node credentials.
//!
//! An administrator mints a one-time join token; the node agent trades
//! it for a node id and a secret of its own. Only hashes are stored, so
//! a database leak does not hand out working node credentials.

use anyhow::Result;
use sqlx::FromRow;

use crate::Db;

/// A join token, without the plaintext secret.
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct EnrollmentTokenRow {
    pub id: i64,
    pub name: String,
    pub node_name: Option<String>,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub used_at_ms: Option<i64>,
    pub used_by_node_id: Option<i64>,
}

impl Db {
    /// Store a new join token. The caller generates the plaintext and
    /// hashes it; only the hash reaches the database.
    pub async fn create_enrollment_token(
        &self,
        name: &str,
        node_name: Option<&str>,
        token_hash: &str,
        created_at_ms: i64,
        expires_at_ms: i64,
    ) -> Result<EnrollmentTokenRow> {
        let row = sqlx::query_as::<_, EnrollmentTokenRow>(
            "INSERT INTO vm_enrollment_tokens
             (name, node_name, token_hash, created_at_ms, expires_at_ms)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id, name, node_name, created_at_ms, expires_at_ms,
                       used_at_ms, used_by_node_id",
        )
        .bind(name)
        .bind(node_name)
        .bind(token_hash)
        .bind(created_at_ms)
        .bind(expires_at_ms)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list_enrollment_tokens(&self) -> Result<Vec<EnrollmentTokenRow>> {
        let rows = sqlx::query_as::<_, EnrollmentTokenRow>(
            "SELECT id, name, node_name, created_at_ms, expires_at_ms,
                    used_at_ms, used_by_node_id
             FROM vm_enrollment_tokens
             ORDER BY id DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Revoke an unused token. Returns whether a row was deleted.
    pub async fn delete_enrollment_token(&self, id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM vm_enrollment_tokens WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// A token that has neither been used nor expired.
    pub async fn find_valid_enrollment_token(
        &self,
        token_hash: &str,
        now_ms: i64,
    ) -> Result<Option<EnrollmentTokenRow>> {
        let row = sqlx::query_as::<_, EnrollmentTokenRow>(
            "SELECT id, name, node_name, created_at_ms, expires_at_ms,
                    used_at_ms, used_by_node_id
             FROM vm_enrollment_tokens
             WHERE token_hash = $1 AND used_at_ms IS NULL AND expires_at_ms > $2",
        )
        .bind(token_hash)
        .bind(now_ms)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Atomically claim a token for a node. Returns false when another
    /// request won the race or the token expired between the lookup and
    /// this call.
    pub async fn consume_enrollment_token(
        &self,
        token_hash: &str,
        now_ms: i64,
        node_id: i64,
    ) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE vm_enrollment_tokens
             SET used_at_ms = $2, used_by_node_id = $3
             WHERE token_hash = $1 AND used_at_ms IS NULL AND expires_at_ms > $2",
        )
        .bind(token_hash)
        .bind(now_ms)
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Attach or rotate the secret a node authenticates with.
    pub async fn set_node_secret(&self, node_id: i64, secret_hash: &str, now_ms: i64) -> Result<()> {
        sqlx::query(
            "INSERT INTO vm_node_credentials (node_id, secret_hash, created_at_ms, updated_at_ms)
             VALUES ($1, $2, $3, $3)
             ON CONFLICT (node_id) DO UPDATE SET secret_hash = excluded.secret_hash,
                                                 updated_at_ms = excluded.updated_at_ms",
        )
        .bind(node_id)
        .bind(secret_hash)
        .bind(now_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Whether `secret_hash` is the current secret for `node_id`.
    pub async fn node_secret_matches(&self, node_id: i64, secret_hash: &str) -> Result<bool> {
        let row = sqlx::query(
            "SELECT 1 FROM vm_node_credentials WHERE node_id = $1 AND secret_hash = $2",
        )
        .bind(node_id)
        .bind(secret_hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    /// Forget a node's secret. Used when a node is removed or rotated.
    pub async fn delete_node_secret(&self, node_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM vm_node_credentials WHERE node_id = $1")
            .bind(node_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// A helper for tests and callers that already have a row.
pub fn token_is_usable(row: &EnrollmentTokenRow, now_ms: i64) -> bool {
    row.used_at_ms.is_none() && row.expires_at_ms > now_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_used_or_expired_token_is_not_usable() {
        let row = EnrollmentTokenRow {
            id: 1,
            name: String::new(),
            node_name: None,
            created_at_ms: 0,
            expires_at_ms: 100,
            used_at_ms: None,
            used_by_node_id: None,
        };
        assert!(token_is_usable(&row, 99));
        assert!(!token_is_usable(&row, 100));

        let used = EnrollmentTokenRow {
            used_at_ms: Some(50),
            ..row
        };
        assert!(!token_is_usable(&used, 99));
    }
}
