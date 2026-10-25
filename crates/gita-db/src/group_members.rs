//! Data access for Gita, split by domain.

use anyhow::Result;
use gita_core::*;
use sqlx::Row;

use crate::Db;

impl Db {
    /// List members of a group, joining with the user table for usernames.
    pub async fn list_group_members(&self, group_id: i64) -> Result<Vec<GroupMember>> {
        let rows = sqlx::query(
            "SELECT gm.group_id, gm.user_id, u.username, gm.role
             FROM group_members gm JOIN users u ON u.id = gm.user_id
             WHERE gm.group_id = $1
             ORDER BY u.username",
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(GroupMember {
                group_id: row.try_get("group_id")?,
                user_id: row.try_get("user_id")?,
                username: row.try_get("username")?,
                role: row.try_get("role")?,
            });
        }
        Ok(out)
    }

    /// Add or update a group's member. Returns `true` on insert, `false`
    /// when updating an existing row.
    pub async fn upsert_group_member(
        &self,
        group_id: i64,
        user_id: i64,
        role: &str,
    ) -> Result<bool> {
        let row = sqlx::query(
            "INSERT INTO group_members (group_id, user_id, role)
             VALUES ($1, $2, $3)
             ON CONFLICT (group_id, user_id) DO UPDATE SET role = EXCLUDED.role
             RETURNING (xmax = 0) AS inserted",
        )
        .bind(group_id)
        .bind(user_id)
        .bind(role)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<bool, _>("inserted").unwrap_or(false))
    }

    /// Remove a user from a group. Returns whether a row was removed.
    pub async fn remove_group_member(&self, group_id: i64, user_id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM group_members WHERE group_id = $1 AND user_id = $2")
            .bind(group_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Return a user's role in a group (or `None` if not a member).
    pub async fn get_group_member_role(
        &self,
        group_id: i64,
        user_id: i64,
    ) -> Result<Option<String>> {
        let rows =
            sqlx::query("SELECT role FROM group_members WHERE group_id = $1 AND user_id = $2")
                .bind(group_id)
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .first()
            .and_then(|r| r.try_get::<String, _>("role").ok()))
    }
}
