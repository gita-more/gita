//! Data access for Gita, split by domain.

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::Db;

#[derive(Debug, Clone)]
pub struct PasswordResetRow {
    pub token: String,
    pub user_id: i64,
    pub expires_at: String,
    pub used: bool,
    pub created_at: String,
}

impl Db {
    /// Store a new password-reset token (caller is responsible for
    /// generating a high-entropy random string).
    pub async fn create_password_reset_token(
        &self,
        user_id: i64,
        token: &str,
        expires_at: &str,
    ) -> Result<()> {
        let created_at = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO password_reset_tokens (token, user_id, expires_at, used, created_at)
             VALUES ($1, $2, $3, FALSE, $4)",
        )
        .bind(token)
        .bind(user_id)
        .bind(expires_at)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Look up a token; returns `None` when missing, expired, or used.
    ///
    /// **Do not use this to redeem a token.** It is a read: it does not
    /// consume the token, so a caller that reads and then marks it used is
    /// racing every other caller doing the same. Use
    /// [`Db::claim_password_reset_token`], which consumes the token in the
    /// same statement that reads it.
    pub async fn get_valid_password_reset_token(
        &self,
        token: &str,
    ) -> Result<Option<PasswordResetRow>> {
        let rows = sqlx::query(
            "SELECT token, user_id, expires_at, used, created_at
             FROM password_reset_tokens WHERE token = $1",
        )
        .bind(token)
        .fetch_all(&self.pool)
        .await?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let r = PasswordResetRow {
            token: row.try_get("token")?,
            user_id: row.try_get("user_id")?,
            expires_at: row.try_get("expires_at")?,
            used: row.try_get("used")?,
            created_at: row.try_get("created_at")?,
        };
        // Best-effort expiry check (parsed later if RFC3339 fails we treat
        // the token as expired).
        let expired = DateTime::parse_from_rfc3339(&r.expires_at)
            .map(|d| d.with_timezone(&Utc) < Utc::now())
            .unwrap_or(true);
        if r.used || expired {
            return Ok(None);
        }
        Ok(Some(r))
    }

    /// Mark the token consumed so it can never be reused.
    ///
    /// **Do not use this to redeem a token.** Marking it used in a separate
    /// statement from the read leaves a window in which a concurrent
    /// request also passes the read and also redeems the token. Use
    /// [`Db::claim_password_reset_token`], which does both at once.
    pub async fn mark_password_reset_token_used(&self, token: &str) -> Result<()> {
        sqlx::query("UPDATE password_reset_tokens SET used = TRUE WHERE token = $1")
            .bind(token)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Atomically claim a reset token, returning the user it belongs to.
    ///
    /// The token is consumed by the same statement that reads it: the
    /// `WHERE` clause requires the row to still be unused and unexpired, and
    /// `RETURNING` hands back the user id. A second concurrent claim of the
    /// same token matches no row and gets `None`.
    ///
    /// This is what makes the token genuinely single-use. Reading it
    /// (`get_valid_password_reset_token`) and marking it used afterwards is
    /// two separate statements, so two concurrent requests could both pass
    /// the read and both redeem the token — the second one would silently
    /// overwrite the password the first one just set.
    ///
    /// `now` is the current time in RFC3339, the format the `expires_at`
    /// column stores. The expiry is compared as `timestamptz`, not as text:
    /// `expires_at` is a TEXT column, and a text comparison of two
    /// timestamps of the same second but different fractional widths orders
    /// them wrongly.
    pub async fn claim_password_reset_token(&self, token: &str, now: &str) -> Result<Option<i64>> {
        let row = sqlx::query(
            "UPDATE password_reset_tokens
             SET used = TRUE
             WHERE token = $1
               AND used = FALSE
               AND expires_at::timestamptz > $2::timestamptz
             RETURNING user_id",
        )
        .bind(token)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.try_get("user_id")).transpose()?)
    }

    /// Delete a user's spent or expired reset tokens.
    ///
    /// Called when a new token is issued. Tokens that have already been
    /// used, or whose expiry has passed, can never be redeemed again — so
    /// removing them keeps the table from growing without bound. Tokens
    /// that are still valid are left alone: issuing a second link must not
    /// silently invalidate the first one.
    ///
    /// `now` is the current time in RFC3339, the same format the `expires_at`
    /// column stores. The expiry is compared as `timestamptz`, not as text:
    /// `expires_at` is a TEXT column, and a text comparison of two
    /// timestamps of the same second but different fractional widths orders
    /// them wrongly — which would purge a token that is still valid.
    ///
    /// Note: the migration comment for this table still says the rows are
    /// "garbage-collected at use time" after 24 hours, but the tokens are
    /// issued with a one-hour expiry and this method is the cleanup that
    /// actually runs. The migration file cannot be edited to correct the
    /// comment — sqlx validates the checksum of migrations it has already
    /// applied — so the accurate description lives here.
    pub async fn purge_stale_password_reset_tokens(&self, user_id: i64, now: &str) -> Result<()> {
        sqlx::query(
            "DELETE FROM password_reset_tokens
             WHERE user_id = $1
               AND (used = TRUE OR expires_at::timestamptz <= $2::timestamptz)",
        )
        .bind(user_id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
