//! Repository access decisions.
//!
//! The rule itself lives in [`gita_core::access`] — it is a pure
//! function, so it can be reasoned about and unit-tested without a
//! database. This module supplies the facts the rule needs (principally
//! the caller's role in the repository's group) and exposes one call
//! that every transport can use.

use crate::Db;
use anyhow::Result;
use gita_core::access::{can_access, Access, GroupRole, Principal};
use gita_core::Repository;
use sqlx::Row;
use std::collections::HashMap;

impl Db {
    /// The caller's role in a group, if they are a member.
    pub async fn group_role(&self, group_id: i64, user_id: i64) -> Result<Option<GroupRole>> {
        Ok(self
            .get_group_member_role(group_id, user_id)
            .await?
            .map(|role| GroupRole::parse(&role)))
    }

    /// The caller's role in every group they belong to, keyed by group id.
    ///
    /// Listing endpoints filter many repositories at once; resolving the
    /// caller's groups once and deciding in memory keeps that from
    /// turning into one query per repository.
    pub async fn group_roles(&self, user_id: i64) -> Result<HashMap<i64, GroupRole>> {
        let rows = sqlx::query("SELECT group_id, role FROM group_members WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?;
        let mut roles = HashMap::new();
        for row in rows {
            let group_id: i64 = row.try_get("group_id")?;
            let role: String = row.try_get("role")?;
            roles.insert(group_id, GroupRole::parse(&role));
        }
        Ok(roles)
    }

    /// Describe the caller as far as this repository is concerned.
    ///
    /// For a personal repository this is just the user id. For a
    /// repository in a group namespace it also carries the caller's role
    /// in that group, if they are a member.
    pub async fn principal_for(
        &self,
        repo: &Repository,
        user_id: Option<i64>,
    ) -> Result<Principal> {
        let group_role = match (repo.group_id, user_id) {
            (Some(group_id), Some(user_id)) => self.group_role(group_id, user_id).await?,
            _ => None,
        };
        Ok(Principal {
            user_id,
            group_role,
        })
    }

    /// Whether the caller may perform `action` on this repository.
    ///
    /// This is the one call every layer should use to decide access.
    /// See [`gita_core::access`] for the rule and for why the transports
    /// report a refusal differently.
    pub async fn can_access(
        &self,
        repo: &Repository,
        user_id: Option<i64>,
        action: Access,
    ) -> Result<bool> {
        let principal = self.principal_for(repo, user_id).await?;
        Ok(can_access(repo, &principal, action))
    }

    /// Keep only the repositories the caller may read.
    ///
    /// The caller's group memberships are resolved once, so filtering a
    /// long list does not issue a query per repository.
    pub async fn filter_readable(
        &self,
        repos: Vec<Repository>,
        user_id: Option<i64>,
    ) -> Result<Vec<Repository>> {
        let roles = match user_id {
            Some(id) => self.group_roles(id).await?,
            None => HashMap::new(),
        };
        Ok(repos
            .into_iter()
            .filter(|repo| {
                let principal = Principal {
                    user_id,
                    group_role: repo
                        .group_id
                        .and_then(|group_id| roles.get(&group_id).copied()),
                };
                can_access(repo, &principal, Access::Read)
            })
            .collect())
    }
}
