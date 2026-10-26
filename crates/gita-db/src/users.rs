//! Data access for Gita, split by domain.

use anyhow::Result;
use chrono::{DateTime, Utc};
use gita_core::*;
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::Db;
use crate::groups::row_to_group;
use crate::repositories::row_to_repo;

impl Db {
    pub async fn create_user(
        &self,
        username: &str,
        email: &str,
        password_hash: &str,
    ) -> Result<User> {
        self.create_user_with_locale(username, email, password_hash, "zh")
            .await
    }

    pub async fn create_user_with_locale(
        &self,
        username: &str,
        email: &str,
        password_hash: &str,
        locale: &str,
    ) -> Result<User> {
        let created_at = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO users (username, email, password_hash, created_at, locale)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id, is_admin",
        )
        .bind(username)
        .bind(email)
        .bind(password_hash)
        .bind(&created_at)
        .bind(locale)
        .fetch_one(&self.pool)
        .await?;
        let id: i64 = row.try_get("id").unwrap_or(0);
        Ok(User {
            id,
            username: username.to_string(),
            email: email.to_string(),
            password_hash: password_hash.to_string(),
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            locale: locale.to_string(),
            is_admin: row.try_get("is_admin").unwrap_or(false),
        })
    }

    pub async fn update_user_locale(&self, user_id: i64, locale: &str) -> Result<Option<User>> {
        let rows = sqlx::query(
            "UPDATE users SET locale = $2 WHERE id = $1
             RETURNING id, username, email, password_hash, created_at, locale, is_admin",
        )
        .bind(user_id)
        .bind(locale)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_user(&rows[0])?))
    }

    pub async fn update_user_email(&self, user_id: i64, email: &str) -> Result<Option<User>> {
        let rows = sqlx::query(
            "UPDATE users SET email = $2 WHERE id = $1
             RETURNING id, username, email, password_hash, created_at, locale, is_admin",
        )
        .bind(user_id)
        .bind(email)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_user(&rows[0])?))
    }

    /// Update a user's password (raw Argon2 hash).
    pub async fn update_user_password_hash(&self, user_id: i64, new_hash: &str) -> Result<bool> {
        let res = sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(user_id)
            .bind(new_hash)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn get_user_by_id(&self, id: i64) -> Result<Option<User>> {
        let rows = sqlx::query(
            "SELECT id, username, email, password_hash, created_at, locale, is_admin FROM users WHERE id = $1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_user(&rows[0])?))
    }

    pub async fn get_user_by_username(&self, username: &str) -> Result<Option<User>> {
        let rows = sqlx::query(
            "SELECT id, username, email, password_hash, created_at, locale, is_admin FROM users WHERE username = $1",
        )
        .bind(username)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_user(&rows[0])?))
    }

    pub async fn get_user_by_email(&self, email: &str) -> Result<Option<User>> {
        let rows = sqlx::query(
            "SELECT id, username, email, password_hash, created_at, locale, is_admin FROM users WHERE email = $1",
        )
        .bind(email)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_user(&rows[0])?))
    }

    /// Return the number of registered users. Used to decide whether the
    /// system has been initialized (first admin account created) yet.
    pub async fn count_users(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*)::bigint AS n FROM users")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("n").unwrap_or(0))
    }

    pub async fn list_users(&self, limit: usize) -> Result<Vec<User>> {
        let rows = sqlx::query("SELECT id, username, email, password_hash, created_at, locale, is_admin FROM users ORDER BY id DESC LIMIT $1")
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_user).collect()
    }

    /// Create a user with an explicit administrator flag. Used by the admin
    /// settings page to provision accounts (and to grant/revoke the admin
    /// role on creation).
    pub async fn create_user_with_role(
        &self,
        username: &str,
        email: &str,
        password_hash: &str,
        locale: &str,
        is_admin: bool,
    ) -> Result<User> {
        let created_at = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO users (username, email, password_hash, created_at, locale, is_admin)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, is_admin",
        )
        .bind(username)
        .bind(email)
        .bind(password_hash)
        .bind(&created_at)
        .bind(locale)
        .bind(is_admin)
        .fetch_one(&self.pool)
        .await?;
        let id: i64 = row.try_get("id").unwrap_or(0);
        Ok(User {
            id,
            username: username.to_string(),
            email: email.to_string(),
            password_hash: password_hash.to_string(),
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            locale: locale.to_string(),
            is_admin: row.try_get("is_admin").unwrap_or(false),
        })
    }

    /// Update a user's administrator flag. Returns `None` if the user does
    /// not exist. Refuses to demote the very last admin: at least one
    /// administrator must always remain so the system stays manageable.
    pub async fn update_user_admin(&self, user_id: i64, is_admin: bool) -> Result<Option<User>> {
        // If we are about to demote this user, count the remaining admins.
        if !is_admin {
            let admins = self.count_admins().await?;
            if admins <= 1 {
                let target = self.get_user_by_id(user_id).await?;
                if target.as_ref().is_some_and(|u| u.is_admin) {
                    return Err(anyhow::anyhow!(
                        "cannot demote the last administrator: at least one admin must remain"
                    ));
                }
            }
        }
        let rows = sqlx::query(
            "UPDATE users SET is_admin = $2 WHERE id = $1
             RETURNING id, username, email, password_hash, created_at, locale, is_admin",
        )
        .bind(user_id)
        .bind(is_admin)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_user(&rows[0])?))
    }

    /// Count how many users have the administrator flag set.
    pub async fn count_admins(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*)::bigint AS n FROM users WHERE is_admin = TRUE")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("n").unwrap_or(0))
    }

    /// List the repositories a user owns (personal repositories plus any
    /// group repositories they created, i.e. where `owner_id = user_id`).
    pub async fn list_repositories_owned_by(&self, user_id: i64) -> Result<Vec<Repository>> {
        let rows = sqlx::query(
            "SELECT id, owner_id, owner_name, name, description, is_private, default_branch, created_at, group_id
             FROM repositories WHERE owner_id = $1 ORDER BY id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_repo).collect()
    }

    /// List the groups a user created (i.e. `owner_id = user_id`).
    pub async fn list_groups_owned_by(&self, user_id: i64) -> Result<Vec<Group>> {
        let rows = sqlx::query(
            "SELECT id, name, description, owner_id, created_at, parent_id, full_path FROM groups WHERE owner_id = $1 ORDER BY id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_group).collect()
    }

    /// Delete a user row. Callers must first remove (or reassign) the data
    /// that references the user without `ON DELETE CASCADE` (repositories,
    /// groups, comments, issues, PRs). Rows that carry `ON DELETE CASCADE`
    /// (access tokens, ssh keys, stars, password-reset tokens) are removed
    /// automatically by the database.
    pub async fn delete_user(&self, user_id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}

pub(crate) fn row_to_user(row: &PgRow) -> Result<User> {
    let created_at: String = row.try_get("created_at")?;
    Ok(User {
        id: row.try_get("id")?,
        username: row.try_get("username")?,
        email: row.try_get("email")?,
        password_hash: row.try_get("password_hash")?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        locale: row.try_get("locale")?,
        is_admin: row.try_get("is_admin").unwrap_or(false),
    })
}
