//! Data access layer for Gita.
//!
//! PostgreSQL-only. Schema is created idempotently at startup.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use gita_core::*;
use serde_json::Value;
use sqlx::postgres::{PgConnection, PgPool, PgPoolOptions, PgRow};
use sqlx::{Connection, Row};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// The database handle used across the application.
#[derive(Debug, Clone)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Open a database from a PostgreSQL URL.
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(10))
            .connect(url)
            .await
            .with_context(|| format!("failed to connect to database {url}"))?;
        // sqlx::migrate! tracks applied versions in `_sqlx_migrations`. Each
        // subsequent schema change should land in crates/gita-db/migrations/
        // as a new <timestamp>_<name>.sql file rather than being patched
        // in-code.
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    /// Open a database connection scoped to a specific Postgres schema.
    ///
    /// Useful for test isolation: each test gets its own schema
    /// (`gita_test_<pid>_<n>`) and this method pins `search_path` to it on
    /// every pooled connection *before* running migrations, so all tables
    /// land in the test schema rather than `public`.
    pub async fn connect_with_schema(url: &str, schema: &str) -> Result<Self> {
        let schema_conn = schema.to_string();
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(move |conn, _meta| {
                let schema = schema_conn.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path TO \"{schema}\""))
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .with_context(|| format!("failed to connect to database {url}"))?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    /// Open an existing pool. Useful for tests that share a connection pool
    /// across multiple schema-isolated tests.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Run a migration for adding new tables in future versions.
    pub async fn raw_query(&self, sql: &str) -> Result<()> {
        sqlx::query(sql).execute(&self.pool).await?;
        Ok(())
    }

    /// Execute an arbitrary SQL statement (used by tests).
    pub async fn execute(&self, sql: &str) -> Result<()> {
        sqlx::query(sql).execute(&self.pool).await?;
        Ok(())
    }

    /// Persist an event to the event log.
    pub async fn insert_event(&self, event: &Event) -> Result<i64> {
        let payload = serde_json::to_string(&event.payload)?;
        let row = sqlx::query(
            "INSERT INTO events (event_type, repo_id, actor_id, payload, created_at)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id",
        )
        .bind(event.event_type.to_string())
        .bind(event.repo_id)
        .bind(event.actor_id)
        .bind(payload)
        .bind(event.created_at.to_rfc3339())
        .fetch_one(&self.pool)
        .await?;
        let id: i64 = row.try_get("id").unwrap_or(0);
        Ok(id)
    }

    /// List recent events from the event log.
    pub async fn list_events(
        &self,
        repo_id: Option<i64>,
        event_type: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Event>> {
        let mut sql = String::from(
            "SELECT id, event_type, repo_id, actor_id, payload, created_at FROM events",
        );
        let mut conds: Vec<&str> = Vec::new();
        if repo_id.is_some() {
            conds.push("repo_id = $2");
        }
        if event_type.is_some() {
            conds.push(if repo_id.is_some() {
                "event_type = $3"
            } else {
                "event_type = $2"
            });
        }
        if !conds.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
        }
        sql.push_str(" ORDER BY id DESC LIMIT $1");
        let mut q = sqlx::query(&sql).bind(limit as i64);
        if let Some(rid) = repo_id {
            q = q.bind(rid);
        }
        if let Some(et) = event_type {
            q = q.bind(et);
        }
        let rows = q.fetch_all(&self.pool).await?;
        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            let payload: String = row.try_get("payload")?;
            let payload: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
            let event_type: String = row.try_get("event_type")?;
            let event_type = parse_event_type(&event_type)?;
            let created_at: String = row.try_get("created_at")?;
            events.push(Event {
                id: row.try_get("id")?,
                event_type,
                repo_id: row.try_get("repo_id")?,
                actor_id: row.try_get("actor_id")?,
                payload,
                created_at: DateTime::parse_from_rfc3339(&created_at)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            });
        }
        Ok(events)
    }

    /// The underlying pool (used for migrations and advanced queries).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

fn parse_event_type(s: &str) -> Result<EventType> {
    Ok(match s {
        "repo_created" => EventType::RepoCreated,
        "repo_deleted" => EventType::RepoDeleted,
        "push" => EventType::Push,
        "pull_request_opened" => EventType::PullRequestOpened,
        "pull_request_merged" => EventType::PullRequestMerged,
        "pull_request_closed" => EventType::PullRequestClosed,
        "issue_created" => EventType::IssueCreated,
        "issue_closed" => EventType::IssueClosed,
        "user_registered" => EventType::UserRegistered,
        "agent_run_started" => EventType::AgentRunStarted,
        "agent_run_completed" => EventType::AgentRunCompleted,
        "pipeline_requested" => EventType::PipelineRequested,
        "pipeline_started" => EventType::PipelineStarted,
        "pipeline_completed" => EventType::PipelineCompleted,
        "pipeline_failed" => EventType::PipelineFailed,
        "job_started" => EventType::JobStarted,
        "job_completed" => EventType::JobCompleted,
        other => return Err(anyhow!("unknown event type: {other}")),
    })
}

// ---------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------

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
             RETURNING id",
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
        })
    }

    pub async fn update_user_locale(&self, user_id: i64, locale: &str) -> Result<Option<User>> {
        let rows = sqlx::query(
            "UPDATE users SET locale = $2 WHERE id = $1
             RETURNING id, username, email, password_hash, created_at, locale",
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
             RETURNING id, username, email, password_hash, created_at, locale",
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
    pub async fn update_user_password_hash(
        &self,
        user_id: i64,
        new_hash: &str,
    ) -> Result<bool> {
        let res = sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(user_id)
            .bind(new_hash)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn get_user_by_id(&self, id: i64) -> Result<Option<User>> {
        let rows = sqlx::query(
            "SELECT id, username, email, password_hash, created_at, locale FROM users WHERE id = $1",
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
            "SELECT id, username, email, password_hash, created_at, locale FROM users WHERE username = $1",
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
            "SELECT id, username, email, password_hash, created_at, locale FROM users WHERE email = $1",
        )
        .bind(email)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_user(&rows[0])?))
    }

    pub async fn list_users(&self, limit: usize) -> Result<Vec<User>> {
        let rows = sqlx::query("SELECT id, username, email, password_hash, created_at, locale FROM users ORDER BY id DESC LIMIT $1")
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_user).collect()
    }
}

fn row_to_user(row: &PgRow) -> Result<User> {
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
    })
}

// ---------------------------------------------------------------------------
// Repositories
// ---------------------------------------------------------------------------

impl Db {
    pub async fn create_repository(
        &self,
        owner_id: i64,
        owner_name: &str,
        name: &str,
        description: &str,
        is_private: bool,
        default_branch: &str,
    ) -> Result<Repository> {
        let created_at = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO repositories (owner_id, owner_name, name, description, is_private, default_branch, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING id",
        )
        .bind(owner_id)
        .bind(owner_name)
        .bind(name)
        .bind(description)
        .bind(is_private as i64)
        .bind(default_branch)
        .bind(&created_at)
        .fetch_one(&self.pool)
        .await?;
        let id: i64 = row.try_get("id").unwrap_or(0);
        Ok(Repository {
            id,
            owner_id,
            owner_name: owner_name.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            is_private,
            default_branch: default_branch.to_string(),
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    }

    pub async fn get_repository(&self, id: i64) -> Result<Option<Repository>> {
        let rows = sqlx::query(
            "SELECT id, owner_id, owner_name, name, description, is_private, default_branch, created_at
             FROM repositories WHERE id = $1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_repo(&rows[0])?))
    }

    pub async fn get_repository_by_name(
        &self,
        owner_name: &str,
        name: &str,
    ) -> Result<Option<Repository>> {
        let rows = sqlx::query(
            "SELECT id, owner_id, owner_name, name, description, is_private, default_branch, created_at
             FROM repositories WHERE owner_name = $1 AND name = $2",
        )
        .bind(owner_name)
        .bind(name)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_repo(&rows[0])?))
    }

    pub async fn list_repositories(
        &self,
        owner_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Repository>> {        let rows = if let Some(owner) = owner_name {
            sqlx::query(
                "SELECT id, owner_id, owner_name, name, description, is_private, default_branch, created_at
                 FROM repositories WHERE owner_name = $2 ORDER BY id DESC LIMIT $1",
            )
            .bind(limit as i64)
            .bind(owner)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT id, owner_id, owner_name, name, description, is_private, default_branch, created_at
                 FROM repositories ORDER BY id DESC LIMIT $1",
            )
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?
        };
        rows.iter().map(row_to_repo).collect()
    }

        pub async fn delete_repository(&self, id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM repositories WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Search public repositories by name or description (ILIKE). Hidden
    /// (`is_private = true`) rows are excluded unless the search query is
    /// empty — callers that need owner-scoped lookups should use the
    /// `?owner=` filter on the listing endpoint instead.
    pub async fn search_repositories(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<Repository>> {
        let pattern = format!("%{query}%");
        let rows = sqlx::query(
            "SELECT id, owner_id, owner_name, name, description, is_private, default_branch, created_at
             FROM repositories
             WHERE (name ILIKE $1 OR description ILIKE $1)
               AND is_private = 0
             ORDER BY id DESC
             LIMIT $2",
        )
        .bind(pattern)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_repo).collect()
    }

    pub async fn update_repository_description(&self, id: i64, description: &str) -> Result<()> {
        sqlx::query("UPDATE repositories SET description = $2 WHERE id = $1")
            .bind(id)
            .bind(description)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Update a repository's visibility (`is_private`).
    pub async fn update_repository_visibility(&self, id: i64, is_private: bool) -> Result<()> {
        sqlx::query("UPDATE repositories SET is_private = $2 WHERE id = $1")
            .bind(id)
            .bind(is_private as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Rename a repository. Returns the updated row.
    pub async fn rename_repository(
        &self,
        id: i64,
        new_owner_name: &str,
        new_name: &str,
    ) -> Result<Option<Repository>> {
        let rows = sqlx::query(
            "UPDATE repositories SET owner_name = $2, name = $3 WHERE id = $1
             RETURNING id, owner_id, owner_name, name, description, is_private, default_branch, created_at, group_id",
        )
        .bind(id)
        .bind(new_owner_name)
        .bind(new_name)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.first().map(row_to_repo).transpose()?)
    }

    /// Transfer a repository to a new owner (user or group).
    pub async fn transfer_repository(
        &self,
        id: i64,
        new_owner_id: i64,
        new_owner_name: &str,
        group_id: Option<i64>,
    ) -> Result<Option<Repository>> {
        let rows = sqlx::query(
            "UPDATE repositories SET owner_id = $2, owner_name = $3, group_id = $4 WHERE id = $1
             RETURNING id, owner_id, owner_name, name, description, is_private, default_branch, created_at, group_id",
        )
        .bind(id)
        .bind(new_owner_id)
        .bind(new_owner_name)
        .bind(group_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.first().map(row_to_repo).transpose()?)
    }
}

fn row_to_repo(row: &PgRow) -> Result<Repository> {
    let created_at: String = row.try_get("created_at")?;
    Ok(Repository {
        id: row.try_get("id")?,
        owner_id: row.try_get("owner_id")?,
        owner_name: row.try_get("owner_name")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        is_private: row.try_get::<i64, _>("is_private")? != 0,
        default_branch: row.try_get("default_branch")?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    })
}

// ---------------------------------------------------------------------------
// Commits
// ---------------------------------------------------------------------------

impl Db {
    pub async fn insert_commit(&self, commit: &Commit) -> Result<i64> {
        sqlx::query(
            "INSERT INTO commits (repo_id, sha, author_name, author_email, message, parent_shas, authored_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (repo_id, sha) DO NOTHING",
        )
        .bind(commit.repo_id)
        .bind(&commit.sha)
        .bind(&commit.author_name)
        .bind(&commit.author_email)
        .bind(&commit.message)
        .bind(serde_json::to_string(&commit.parent_shas)?)
        .bind(commit.authored_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(0) // ON CONFLICT DO NOTHING does not return id; callers that need id
              // should re-query via get_commit. insert_commit is fire-and-forget
              // for the changeset-walk path.
    }

    pub async fn get_commit(&self, repo_id: i64, sha: &str) -> Result<Option<Commit>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, sha, author_name, author_email, message, parent_shas, authored_at
             FROM commits WHERE repo_id = $1 AND sha = $2",
        )
        .bind(repo_id)
        .bind(sha)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_commit(&rows[0])?))
    }

    pub async fn list_commits(&self, repo_id: i64, limit: usize) -> Result<Vec<Commit>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, sha, author_name, author_email, message, parent_shas, authored_at
             FROM commits WHERE repo_id = $1 ORDER BY authored_at DESC LIMIT $2",
        )
        .bind(repo_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_commit).collect()
    }

    pub async fn latest_commit_sha(&self, repo_id: i64) -> Result<Option<String>> {
        let rows = sqlx::query(
            "SELECT sha FROM commits WHERE repo_id = $1 ORDER BY authored_at DESC LIMIT 1",
        )
        .bind(repo_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.first().and_then(|r| r.try_get("sha").ok()))
    }
}

fn row_to_commit(row: &PgRow) -> Result<Commit> {
    let parent_shas: String = row.try_get("parent_shas")?;
    let authored_at: String = row.try_get("authored_at")?;
    Ok(Commit {
        id: row.try_get("id")?,
        repo_id: row.try_get("repo_id")?,
        sha: row.try_get("sha")?,
        author_name: row.try_get("author_name")?,
        author_email: row.try_get("author_email")?,
        message: row.try_get("message")?,
        parent_shas: serde_json::from_str(&parent_shas).unwrap_or_default(),
        authored_at: DateTime::parse_from_rfc3339(&authored_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    })
}

// ---------------------------------------------------------------------------
// Pull requests
// ---------------------------------------------------------------------------

impl Db {
    pub async fn create_pull_request(
        &self,
        repo_id: i64,
        title: &str,
        body: &str,
        source_branch: &str,
        target_branch: &str,
        author_id: i64,
        author_name: &str,
        base_sha: &str,
        head_sha: &str,
    ) -> Result<PullRequest> {
        let now = Utc::now().to_rfc3339();
        // Compute next number
        let rows = sqlx::query("SELECT COALESCE(MAX(number), 0) + 1 AS next_num FROM pull_requests WHERE repo_id = $1")
            .bind(repo_id)
            .fetch_all(&self.pool)
            .await?;
        let number: i64 = rows
            .first()
            .map(|r| r.try_get::<i64, _>("next_num").unwrap_or(1))
            .unwrap_or(1);
        sqlx::query(
            "INSERT INTO pull_requests (repo_id, number, title, body, source_branch, target_branch, status, author_id, author_name, base_sha, head_sha, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, 'open', $7, $8, $9, $10, $11, $12)",
        )
        .bind(repo_id)
        .bind(number)
        .bind(title)
        .bind(body)
        .bind(source_branch)
        .bind(target_branch)
        .bind(author_id)
        .bind(author_name)
        .bind(base_sha)
        .bind(head_sha)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(PullRequest {
            id: 0, // not returned by INSERT (no RETURNING); re-fetch via get_pull_request
            repo_id,
            number,
            title: title.to_string(),
            body: body.to_string(),
            source_branch: source_branch.to_string(),
            target_branch: target_branch.to_string(),
            status: PrStatus::Open,
            author_id,
            author_name: author_name.to_string(),
            base_sha: base_sha.to_string(),
            head_sha: head_sha.to_string(),
            created_at: DateTime::parse_from_rfc3339(&now)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            updated_at: DateTime::parse_from_rfc3339(&now)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            merged_at: None,
        })
    }

    pub async fn get_pull_request(&self, repo_id: i64, number: i64) -> Result<Option<PullRequest>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, number, title, body, source_branch, target_branch, status, author_id, author_name, base_sha, head_sha, created_at, updated_at, merged_at
             FROM pull_requests WHERE repo_id = $1 AND number = $2",
        )
        .bind(repo_id)
        .bind(number)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_pr(&rows[0])?))
    }

    pub async fn get_pull_request_by_id(&self, id: i64) -> Result<Option<PullRequest>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, number, title, body, source_branch, target_branch, status, author_id, author_name, base_sha, head_sha, created_at, updated_at, merged_at
             FROM pull_requests WHERE id = $1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_pr(&rows[0])?))
    }

    pub async fn list_pull_requests(
        &self,
        repo_id: i64,
        status: Option<PrStatus>,
        limit: usize,
    ) -> Result<Vec<PullRequest>> {
        let rows = if let Some(st) = status {
            sqlx::query(
                "SELECT id, repo_id, number, title, body, source_branch, target_branch, status, author_id, author_name, base_sha, head_sha, created_at, updated_at, merged_at
                 FROM pull_requests WHERE repo_id = $1 AND status = $3 ORDER BY id DESC LIMIT $2",
            )
            .bind(repo_id)
            .bind(limit as i64)
            .bind(st.to_string())
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT id, repo_id, number, title, body, source_branch, target_branch, status, author_id, author_name, base_sha, head_sha, created_at, updated_at, merged_at
                 FROM pull_requests WHERE repo_id = $1 ORDER BY id DESC LIMIT $2",
            )
            .bind(repo_id)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?
        };
        rows.iter().map(row_to_pr).collect()
    }

    pub async fn update_pr_status(&self, id: i64, status: PrStatus, merged_at: Option<DateTime<Utc>>) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE pull_requests SET status = $2, updated_at = $3, merged_at = $4 WHERE id = $1")
            .bind(id)
            .bind(status.to_string())
            .bind(&now)
            .bind(merged_at.map(|d| d.to_rfc3339()))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_pr_head_sha(&self, id: i64, head_sha: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE pull_requests SET head_sha = $2, updated_at = $3 WHERE id = $1")
            .bind(id)
            .bind(head_sha)
            .bind(&now)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn row_to_pr(row: &PgRow) -> Result<PullRequest> {
    let status: String = row.try_get("status")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;
    let merged_at: Option<String> = row.try_get("merged_at")?;
    Ok(PullRequest {
        id: row.try_get("id")?,
        repo_id: row.try_get("repo_id")?,
        number: row.try_get("number")?,
        title: row.try_get("title")?,
        body: row.try_get("body")?,
        source_branch: row.try_get("source_branch")?,
        target_branch: row.try_get("target_branch")?,
        status: match status.as_str() {
            "merged" => PrStatus::Merged,
            "closed" => PrStatus::Closed,
            _ => PrStatus::Open,
        },
        author_id: row.try_get("author_id")?,
        author_name: row.try_get("author_name")?,
        base_sha: row.try_get("base_sha")?,
        head_sha: row.try_get("head_sha")?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        merged_at: merged_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc)),
    })
}

// ---------------------------------------------------------------------------
// Issues
// ---------------------------------------------------------------------------

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
        let rows = sqlx::query("SELECT COALESCE(MAX(number), 0) + 1 AS next_num FROM issues WHERE repo_id = $1")
            .bind(repo_id)
            .fetch_all(&self.pool)
            .await?;
        let number: i64 = rows
            .first()
            .map(|r| r.try_get::<i64, _>("next_num").unwrap_or(1))
            .unwrap_or(1);
        sqlx::query(
            "INSERT INTO issues (repo_id, number, title, body, status, author_id, author_name, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'open', $5, $6, $7, $8)",
        )
        .bind(repo_id)
        .bind(number)
        .bind(title)
        .bind(body)
        .bind(author_id)
        .bind(author_name)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(Issue {
            id: 0,
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

    pub async fn list_issues(&self, repo_id: i64, status: Option<IssueStatus>, limit: usize) -> Result<Vec<Issue>> {
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

// ---------------------------------------------------------------------------
// Group members
// ---------------------------------------------------------------------------

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
        let rows = sqlx::query(
            "SELECT role FROM group_members WHERE group_id = $1 AND user_id = $2",
        )
        .bind(group_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .first()
            .and_then(|r| r.try_get::<String, _>("role").ok()))
    }
}

// ---------------------------------------------------------------------------
// Comments (PR + Issue)
// ---------------------------------------------------------------------------

impl Db {
    /// Add a comment to a pull request. `author_id`/`author_name` are the
    /// authenticated user; `repo_id` is denormalized for cascade deletes.
    pub async fn create_pr_comment(
        &self,
        repo_id: i64,
        pr_id: i64,
        author_id: i64,
        author_name: &str,
        body: &str,
    ) -> Result<Comment> {
        let now = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO comments (repo_id, pr_id, issue_id, author_id, author_name, body, created_at, updated_at)
             VALUES ($1, $2, NULL, $3, $4, $5, $6, $6)
             RETURNING id, repo_id, pr_id, issue_id, author_id, author_name, body, created_at, updated_at",
        )
        .bind(repo_id)
        .bind(pr_id)
        .bind(author_id)
        .bind(author_name)
        .bind(body)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row_to_comment(&row)?)
    }

    /// Add a comment to an issue.
    pub async fn create_issue_comment(
        &self,
        repo_id: i64,
        issue_id: i64,
        author_id: i64,
        author_name: &str,
        body: &str,
    ) -> Result<Comment> {
        let now = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO comments (repo_id, pr_id, issue_id, author_id, author_name, body, created_at, updated_at)
             VALUES ($1, NULL, $2, $3, $4, $5, $6, $6)
             RETURNING id, repo_id, pr_id, issue_id, author_id, author_name, body, created_at, updated_at",
        )
        .bind(repo_id)
        .bind(issue_id)
        .bind(author_id)
        .bind(author_name)
        .bind(body)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row_to_comment(&row)?)
    }

    /// List comments for a pull request, oldest first.
    pub async fn list_pr_comments(&self, pr_id: i64, limit: usize) -> Result<Vec<Comment>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, pr_id, issue_id, author_id, author_name, body, created_at, updated_at
             FROM comments WHERE pr_id = $1 ORDER BY id ASC LIMIT $2",
        )
        .bind(pr_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_comment).collect()
    }

    /// List comments for an issue, oldest first.
    pub async fn list_issue_comments(&self, issue_id: i64, limit: usize) -> Result<Vec<Comment>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, pr_id, issue_id, author_id, author_name, body, created_at, updated_at
             FROM comments WHERE issue_id = $1 ORDER BY id ASC LIMIT $2",
        )
        .bind(issue_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_comment).collect()
    }

    /// Get a single comment by id.
    pub async fn get_comment(&self, id: i64) -> Result<Option<Comment>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, pr_id, issue_id, author_id, author_name, body, created_at, updated_at
             FROM comments WHERE id = $1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.first().map(row_to_comment).transpose()?)
    }

    /// Delete a comment. Returns whether a row was deleted.
    pub async fn delete_comment(&self, id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM comments WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}

// ---------------------------------------------------------------------------
// Password reset tokens
// ---------------------------------------------------------------------------

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
    pub async fn mark_password_reset_token_used(&self, token: &str) -> Result<()> {
        sqlx::query("UPDATE password_reset_tokens SET used = TRUE WHERE token = $1")
            .bind(token)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Webhooks
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Stars
// ---------------------------------------------------------------------------

impl Db {
    /// Add (or no-op) a star from `user_id` to `repo_id`. Returns true on
    /// insert, false when the row already existed.
    pub async fn add_star(&self, user_id: i64, repo_id: i64) -> Result<bool> {
        let created_at = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO stars (user_id, repo_id, created_at) VALUES ($1, $2, $3)
             ON CONFLICT (user_id, repo_id) DO NOTHING
             RETURNING user_id",
        )
        .bind(user_id)
        .bind(repo_id)
        .bind(&created_at)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    /// Remove a star. Returns whether a row was deleted.
    pub async fn remove_star(&self, user_id: i64, repo_id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM stars WHERE user_id = $1 AND repo_id = $2")
            .bind(user_id)
            .bind(repo_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Whether `user_id` has starred `repo_id`.
    pub async fn has_star(&self, user_id: i64, repo_id: i64) -> Result<bool> {
        let rows = sqlx::query("SELECT 1 AS ok FROM stars WHERE user_id = $1 AND repo_id = $2")
            .bind(user_id)
            .bind(repo_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(!rows.is_empty())
    }

    /// Total star count for a repository.
    pub async fn count_stars(&self, repo_id: i64) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*)::bigint AS n FROM stars WHERE repo_id = $1")
            .bind(repo_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("n").unwrap_or(0))
    }
}

fn row_to_comment(row: &PgRow) -> Result<Comment> {
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;
    Ok(Comment {
        id: row.try_get("id")?,
        repo_id: row.try_get("repo_id")?,
        pr_id: row.try_get("pr_id")?,
        issue_id: row.try_get("issue_id")?,
        author_id: row.try_get("author_id")?,
        author_name: row.try_get("author_name")?,
        body: row.try_get("body")?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    })
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
        status: if status == "closed" { IssueStatus::Closed } else { IssueStatus::Open },
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

// ---------------------------------------------------------------------------
// Access tokens
// ---------------------------------------------------------------------------

impl Db {
    pub async fn create_access_token(
        &self,
        user_id: i64,
        token: &str,
        name: &str,
    ) -> Result<AccessToken> {
        let created_at = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO access_tokens (user_id, token, name, created_at) VALUES ($1, $2, $3, $4)
             RETURNING id",
        )
        .bind(user_id)
        .bind(token)
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

    pub async fn get_user_by_token(&self, token: &str) -> Result<Option<User>> {
        let rows = sqlx::query(
            "SELECT u.id, u.username, u.email, u.password_hash, u.created_at, u.locale
             FROM access_tokens t JOIN users u ON u.id = t.user_id
             WHERE t.token = $1",
        )
        .bind(token)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_user(&rows[0])?))
    }

    pub async fn list_tokens(&self, user_id: i64) -> Result<Vec<AccessToken>> {
        let rows = sqlx::query(
            "SELECT id, user_id, token, name, created_at FROM access_tokens WHERE user_id = $1 ORDER BY id DESC",
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
                token: row.try_get("token")?,
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

/// Parse a datetime stored as RFC3339 text into a `DateTime<Utc>`.
pub fn parse_rfc3339(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

// ---------------------------------------------------------------------------
// Groups
// ---------------------------------------------------------------------------

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
            "SELECT id, owner_id, owner_name, name, description, is_private, default_branch, created_at
             FROM repositories WHERE group_id = $1 ORDER BY name",
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_repo).collect()
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
                        r.is_private, r.default_branch, r.created_at AS repo_created_at
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
                        r.is_private, r.default_branch, r.created_at AS repo_created_at
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

/// Parse an Issue row using the aliased column names produced by
/// `list_all_issues`.
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
        status: if status == "closed" { IssueStatus::Closed } else { IssueStatus::Open },
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

/// Parse a Repository row using the aliased column names produced by
/// `list_all_issues`.
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
    })
}

fn row_to_group(row: &PgRow) -> Result<Group> {
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

// ---------------------------------------------------------------------------
// Pipelines (in-repo CI, Phase 1)
// ---------------------------------------------------------------------------

/// Row shape for the `pipelines` table.
#[derive(Debug, Clone)]
pub struct PipelineRow {
    pub id: i64,
    pub repo_id: i64,
    pub path: String,
    pub ref_name: String,
    pub ref_sha: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Row shape for the `pipeline_runs` table.
#[derive(Debug, Clone)]
pub struct PipelineRunRow {
    pub id: i64,
    pub pipeline_id: i64,
    pub repo_id: i64,
    pub ref_name: String,
    pub head_sha: String,
    pub trigger_event_id: Option<i64>,
    pub trigger_pusher_id: Option<i64>,
    pub status: String,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub claimed_by_runner_id: Option<String>,
    pub claimed_until: Option<String>,
}

/// Row shape for the `jobs` table. `needs` and `script` are JSON-encoded
/// arrays of strings — decoded by callers that need them.
#[derive(Debug, Clone)]
pub struct JobRow {
    pub id: i64,
    pub run_id: i64,
    pub name: String,
    pub stage: String,
    pub needs: Vec<String>,
    pub script: Vec<String>,
    pub status: String,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
}

/// Row shape for the `logs` table.
#[derive(Debug, Clone)]
pub struct LogRow {
    pub id: i64,
    pub job_id: i64,
    pub seq: i64,
    pub stream: String,
    pub line: String,
    pub ts: String,
}

fn row_to_pipeline(row: &PgRow) -> Result<PipelineRow> {
    Ok(PipelineRow {
        id: row.try_get("id")?,
        repo_id: row.try_get("repo_id")?,
        path: row.try_get("path")?,
        ref_name: row.try_get("ref")?,
        ref_sha: row.try_get("ref_sha")?,
        content: row.try_get("content")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_run(row: &PgRow) -> Result<PipelineRunRow> {
    Ok(PipelineRunRow {
        id: row.try_get("id")?,
        pipeline_id: row.try_get("pipeline_id")?,
        repo_id: row.try_get("repo_id")?,
        ref_name: row.try_get("ref")?,
        head_sha: row.try_get("head_sha")?,
        trigger_event_id: row.try_get("trigger_event_id")?,
        trigger_pusher_id: row.try_get("trigger_pusher_id")?,
        status: row.try_get("status")?,
        queued_at: row.try_get("queued_at")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        claimed_by_runner_id: row.try_get("claimed_by_runner_id")?,
        claimed_until: row.try_get("claimed_until")?,
    })
}

fn row_to_job(row: &PgRow) -> Result<JobRow> {
    let needs_json: String = row.try_get("needs")?;
    let script_json: String = row.try_get("script")?;
    let needs: Vec<String> = serde_json::from_str(&needs_json).unwrap_or_default();
    let script: Vec<String> = serde_json::from_str(&script_json).unwrap_or_default();
    Ok(JobRow {
        id: row.try_get("id")?,
        run_id: row.try_get("run_id")?,
        name: row.try_get("name")?,
        stage: row.try_get("stage")?,
        needs,
        script,
        status: row.try_get("status")?,
        queued_at: row.try_get("queued_at")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        exit_code: row.try_get("exit_code")?,
    })
}

fn row_to_log(row: &PgRow) -> Result<LogRow> {
    Ok(LogRow {
        id: row.try_get("id")?,
        job_id: row.try_get("job_id")?,
        seq: row.try_get("seq")?,
        stream: row.try_get("stream")?,
        line: row.try_get("line")?,
        ts: row.try_get("ts")?,
    })
}

impl Db {
    /// Insert-or-update a pipeline record by (repo_id, path). Returns the
    /// row's id. `created_at` is only set on insert; `updated_at` is
    /// refreshed on every call.
    pub async fn upsert_pipeline(
        &self,
        repo_id: i64,
        path: &str,
        ref_name: &str,
        ref_sha: &str,
        content: &str,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO pipelines (repo_id, path, ref, ref_sha, content, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $6)
             ON CONFLICT (repo_id, path) DO UPDATE
                 SET ref = EXCLUDED.ref,
                     ref_sha = EXCLUDED.ref_sha,
                     content = EXCLUDED.content,
                     updated_at = EXCLUDED.updated_at
             RETURNING id",
        )
        .bind(repo_id)
        .bind(path)
        .bind(ref_name)
        .bind(ref_sha)
        .bind(content)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("id").unwrap_or(0))
    }

    pub async fn get_pipeline_by_path(
        &self,
        repo_id: i64,
        path: &str,
    ) -> Result<Option<PipelineRow>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, path, ref, ref_sha, content, created_at, updated_at
             FROM pipelines WHERE repo_id = $1 AND path = $2",
        )
        .bind(repo_id)
        .bind(path)
        .fetch_all(&self.pool)
        .await?;
        rows.first().map(row_to_pipeline).transpose()
    }

    pub async fn get_pipeline(&self, id: i64) -> Result<Option<PipelineRow>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, path, ref, ref_sha, content, created_at, updated_at
             FROM pipelines WHERE id = $1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.first().map(row_to_pipeline).transpose()
    }

    pub async fn insert_pipeline_run(
        &self,
        pipeline_id: i64,
        repo_id: i64,
        ref_name: &str,
        head_sha: &str,
        pusher_id: Option<i64>,
        trigger_event_id: Option<i64>,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO pipeline_runs
               (pipeline_id, repo_id, ref, head_sha, trigger_event_id, trigger_pusher_id,
                status, queued_at)
             VALUES ($1, $2, $3, $4, $5, $6, 'queued', $7)
             RETURNING id",
        )
        .bind(pipeline_id)
        .bind(repo_id)
        .bind(ref_name)
        .bind(head_sha)
        .bind(trigger_event_id)
        .bind(pusher_id)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("id").unwrap_or(0))
    }

    pub async fn list_pipeline_runs(
        &self,
        pipeline_id: i64,
        limit: i64,
    ) -> Result<Vec<PipelineRunRow>> {
        let rows = sqlx::query(
            "SELECT id, pipeline_id, repo_id, ref, head_sha, trigger_event_id,
                    trigger_pusher_id, status, queued_at, started_at, finished_at,
                    claimed_by_runner_id, claimed_until
             FROM pipeline_runs
             WHERE pipeline_id = $1
             ORDER BY id DESC
             LIMIT $2",
        )
        .bind(pipeline_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_run).collect()
    }

    pub async fn get_pipeline_run(&self, id: i64) -> Result<Option<PipelineRunRow>> {
        let rows = sqlx::query(
            "SELECT id, pipeline_id, repo_id, ref, head_sha, trigger_event_id,
                    trigger_pusher_id, status, queued_at, started_at, finished_at,
                    claimed_by_runner_id, claimed_until
             FROM pipeline_runs WHERE id = $1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.first().map(row_to_run).transpose()
    }

    /// Atomically claim the next queued run for `runner_id`. Returns the
    /// claimed run with status now 'running' and `claimed_until` set.
    /// Returns `Ok(None)` if no run is available right now.
    ///
    /// Uses a CTE + FOR UPDATE SKIP LOCKED so multiple runners in
    /// different processes don't double-claim the same row.
    pub async fn claim_pipeline_run(
        &self,
        runner_id: &str,
        lease_seconds: i64,
    ) -> Result<Option<PipelineRunRow>> {
        let now = Utc::now().to_rfc3339();
        let lease_until = (Utc::now() + chrono::Duration::seconds(lease_seconds)).to_rfc3339();
        let rows = sqlx::query(
            "WITH next AS (
                 SELECT id FROM pipeline_runs
                 WHERE status = 'queued'
                   AND (claimed_until IS NULL OR claimed_until < $1)
                 ORDER BY queued_at
                 LIMIT 1
                 FOR UPDATE SKIP LOCKED
             )
             UPDATE pipeline_runs pr
             SET status = 'running',
                 started_at = $2,
                 claimed_by_runner_id = $3,
                 claimed_until = $4
             FROM next
             WHERE pr.id = next.id
             RETURNING pr.id, pr.pipeline_id, pr.repo_id, pr.ref, pr.head_sha,
                       pr.trigger_event_id, pr.trigger_pusher_id, pr.status,
                       pr.queued_at, pr.started_at, pr.finished_at,
                       pr.claimed_by_runner_id, pr.claimed_until",
        )
        .bind(&now)
        .bind(&now)
        .bind(runner_id)
        .bind(&lease_until)
        .fetch_all(&self.pool)
        .await?;
        rows.first().map(row_to_run).transpose()
    }

    /// Update a run's status. If `finished` is true, also stamps
    /// `finished_at` and clears `claimed_until` so the partial-restart
    /// path doesn't try to reclaim.
    pub async fn update_pipeline_run_status(
        &self,
        id: i64,
        status: &str,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let terminal = matches!(status, "success" | "failed" | "canceled");
        if terminal {
            sqlx::query(
                "UPDATE pipeline_runs
                 SET status = $2,
                     finished_at = COALESCE(finished_at, $3),
                     claimed_until = NULL
                 WHERE id = $1",
            )
            .bind(id)
            .bind(status)
            .bind(&now)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query("UPDATE pipeline_runs SET status = $2 WHERE id = $1")
                .bind(id)
                .bind(status)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    pub async fn insert_job(
        &self,
        run_id: i64,
        name: &str,
        stage: &str,
        needs: &[String],
        script: &[String],
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let needs_json = serde_json::to_string(needs).unwrap_or_else(|_| "[]".into());
        let script_json = serde_json::to_string(script).unwrap_or_else(|_| "[]".into());
        let row = sqlx::query(
            "INSERT INTO jobs (run_id, name, stage, needs, script, status, queued_at)
             VALUES ($1, $2, $3, $4, $5, 'queued', $6)
             RETURNING id",
        )
        .bind(run_id)
        .bind(name)
        .bind(stage)
        .bind(needs_json)
        .bind(script_json)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("id").unwrap_or(0))
    }

    pub async fn update_job_status(
        &self,
        id: i64,
        status: &str,
        exit_code: Option<i32>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let terminal = matches!(status, "success" | "failed" | "skipped" | "canceled");
        if terminal {
            sqlx::query(
                "UPDATE jobs
                 SET status = $2,
                     exit_code = $3,
                     finished_at = COALESCE(finished_at, $4)
                 WHERE id = $1",
            )
            .bind(id)
            .bind(status)
            .bind(exit_code)
            .bind(&now)
            .execute(&self.pool)
            .await?;
        } else if status == "running" {
            sqlx::query(
                "UPDATE jobs
                 SET status = $2,
                     started_at = COALESCE(started_at, $3)
                 WHERE id = $1",
            )
            .bind(id)
            .bind(status)
            .bind(&now)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query("UPDATE jobs SET status = $2 WHERE id = $1")
                .bind(id)
                .bind(status)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    pub async fn list_jobs(&self, run_id: i64) -> Result<Vec<JobRow>> {
        let rows = sqlx::query(
            "SELECT id, run_id, name, stage, needs, script, status, queued_at,
                    started_at, finished_at, exit_code
             FROM jobs WHERE run_id = $1 ORDER BY id",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_job).collect()
    }

    pub async fn get_job(&self, id: i64) -> Result<Option<JobRow>> {
        let rows = sqlx::query(
            "SELECT id, run_id, name, stage, needs, script, status, queued_at,
                    started_at, finished_at, exit_code
             FROM jobs WHERE id = $1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.first().map(row_to_job).transpose()
    }

    pub async fn insert_log(
        &self,
        job_id: i64,
        seq: i64,
        stream: &str,
        line: &str,
        ts: &str,
    ) -> Result<i64> {
        let row = sqlx::query(
            "INSERT INTO logs (job_id, seq, stream, line, ts)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (job_id, seq) DO NOTHING
             RETURNING id",
        )
        .bind(job_id)
        .bind(seq)
        .bind(stream)
        .bind(line)
        .bind(ts)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("id").unwrap_or(0))
    }

    /// Fetch log lines for a job, starting at `since_seq` (inclusive).
    /// Pass `since_seq = -1` for the first page.
    pub async fn list_logs_since(
        &self,
        job_id: i64,
        since_seq: i64,
        max: i64,
    ) -> Result<Vec<LogRow>> {
        let rows = sqlx::query(
            "SELECT id, job_id, seq, stream, line, ts
             FROM logs
             WHERE job_id = $1 AND seq >= $2
             ORDER BY seq
             LIMIT $3",
        )
        .bind(job_id)
        .bind(since_seq)
        .bind(max)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_log).collect()
    }
}

#[allow(dead_code)]
fn _assert_send_sync(_db: &Db) {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Db>();
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Counter ensuring every test schema gets a unique name.
static TEST_SCHEMA_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Build a `Db` connected to a per-test, schema-isolated Postgres.
///
/// `url` should be a base Postgres URL like `postgres://gita:gita@localhost:55432/gita`.
/// Each call derives a unique schema name (`gita_test_<pid>_<n>`), sets
/// `search_path` to it, runs `sqlx::migrate!`, and returns the Db. The schema is
/// left behind on the server — `drop_test_db` cleans it up after the test.
pub async fn new_test_db(url: &str) -> Db {
    let schema = format!(
        "gita_test_{}_{}",
        std::process::id(),
        TEST_SCHEMA_COUNTER.fetch_add(1, Ordering::SeqCst)
    );

    // Create the schema on a throwaway connection. Then connect_with_schema
    // pins search_path on every pooled connection via after_connect so
    // migrations land in the test schema.
    let mut admin = sqlx::PgConnection::connect(url)
        .await
        .expect("failed to connect to test database (admin)");
    sqlx::query(&format!("CREATE SCHEMA \"{schema}\""))
        .execute(&mut admin)
        .await
        .expect("failed to create test schema");

    Db::connect_with_schema(url, &schema)
        .await
        .expect("failed to run migrations")
}

/// Drop the schema backing a test Db.
pub async fn drop_test_db(db: &Db) {
    let schema = format!(
        "gita_test_{}_{}", // reconstructed best-effort; actual lookup below
        std::process::id(),
        0 // intentionally unused; see real lookup in the body
    );
    let _ = schema;
    // The cleanest way is to drop the schema whose name matches search_path.
    // Postgres exposes `current_schema()`; fall back to it.
    let row = sqlx::query("SELECT current_schema() AS s")
        .fetch_one(db.pool())
        .await
        .expect("failed to read current_schema");
    let current: String = row.try_get("s").unwrap_or_default();
    if current.starts_with("gita_test_") {
        let _ = sqlx::query(&format!("DROP SCHEMA \"{current}\" CASCADE"))
            .execute(db.pool())
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn url() -> String {
        env::var("GITA_TEST_DATABASE_URL").unwrap_or_else(|_| {
            "postgres://gita:gita@127.0.0.1:55432/gita".to_string()
        })
    }

    async fn fresh_db() -> Db {
        let db = new_test_db(&url()).await;
        db
    }

    #[tokio::test]
    async fn user_crud() {
        let db = fresh_db().await;
        let u = db
            .create_user("alice", "alice@example.com", "hash")
            .await
            .unwrap();
        assert!(u.id > 0);
        assert_eq!(u.locale, "zh", "default locale should be zh");
        let fetched = db.get_user_by_username("alice").await.unwrap().unwrap();
        assert_eq!(fetched.id, u.id);
        assert_eq!(fetched.locale, "zh");
        assert_eq!(db.get_user_by_username("nobody").await.unwrap(), None);

        // Update locale and verify it persists.
        let updated = db
            .update_user_locale(u.id, "en")
            .await
            .unwrap()
            .expect("user exists");
        assert_eq!(updated.locale, "en");
        let fetched2 = db.get_user_by_username("alice").await.unwrap().unwrap();
        assert_eq!(fetched2.locale, "en");
        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn repo_crud() {
        let db = fresh_db().await;
        let u = db.create_user("bob", "bob@example.com", "h").await.unwrap();
        let r = db
            .create_repository(u.id, "bob", "hello", "desc", false, "main")
            .await
            .unwrap();
        assert!(r.id > 0);
        let fetched = db.get_repository_by_name("bob", "hello").await.unwrap().unwrap();
        assert_eq!(fetched.full_name(), "bob/hello");
        assert!(!fetched.is_private);
        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn group_crud() {
        let db = fresh_db().await;
        let owner = db
            .create_user("groupowner", "groupowner@example.com", "h")
            .await
            .unwrap();
        let g = db
            .create_group("acme", "The Acme group", owner.id)
            .await
            .unwrap();
        assert!(g.id > 0);
        assert_eq!(g.name, "acme");
        assert_eq!(g.description, "The Acme group");
        assert_eq!(g.owner_id, owner.id);

        // Fetch by name
        let fetched = db.get_group_by_name("acme").await.unwrap().unwrap();
        assert_eq!(fetched.id, g.id);

        // List groups for owner (owner is auto member)
        let groups = db.list_groups_for_user(owner.id).await.unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "acme");

        // Create a repo inside the group namespace, then list it.
        let repo = db
            .create_repository(owner.id, "acme", "widgets", "widget repo", false, "main")
            .await
            .unwrap();
        let group_repos = db.list_group_repositories(g.id).await.unwrap();
        assert_eq!(group_repos.len(), 0, "repo not attached to group yet");

        // Attach repo to group.
        sqlx::query("UPDATE repositories SET group_id = $1 WHERE id = $2")
            .bind(g.id)
            .bind(repo.id)
            .execute(db.pool())
            .await
            .unwrap();
        let group_repos = db.list_group_repositories(g.id).await.unwrap();
        assert_eq!(group_repos.len(), 1);
        assert_eq!(group_repos[0].full_name(), "acme/widgets");

        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn list_all_issues_joins_repo() {
        let db = fresh_db().await;
        let owner = db
            .create_user("issueowner", "issueowner@example.com", "h")
            .await
            .unwrap();
        let repo = db
            .create_repository(owner.id, "issueowner", "issues-repo", "desc", false, "main")
            .await
            .unwrap();
        let _issue = db
            .create_issue(repo.id, "Bug found", "details", owner.id, &owner.username)
            .await
            .unwrap();

        let all = db.list_all_issues(None, 50).await.unwrap();
        assert_eq!(all.len(), 1);
        let (issue, r) = &all[0];
        assert_eq!(issue.title, "Bug found");
        assert_eq!(r.name, "issues-repo");
        assert_eq!(r.owner_name, "issueowner");

        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn event_insert_and_list() {
        let db = fresh_db().await;
        let event = Event {
            id: 0,
            event_type: EventType::RepoCreated,
            repo_id: Some(1),
            actor_id: Some(2),
            payload: serde_json::json!({"name": "test"}),
            created_at: Utc::now(),
        };
        let id = db.insert_event(&event).await.unwrap();
        assert!(id > 0);
        let events = db.list_events(None, None, 10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::RepoCreated);
        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn migration_idempotent() {
        let db = fresh_db().await;
        // Re-running migrations against an already-applied schema must succeed
        // (sqlx::migrate! skips applied versions).
        sqlx::migrate!("./migrations")
            .run(db.pool())
            .await
            .unwrap();
        // _sqlx_migrations must contain entries for both init and groups.
        let rows: Vec<(i64, String)> =
            sqlx::query_as("SELECT version, description FROM _sqlx_migrations ORDER BY version")
                .fetch_all(db.pool())
                .await
                .unwrap();
        let descriptions: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
        assert!(
            descriptions.contains(&"init") && descriptions.contains(&"groups"),
            "expected init + groups migrations, got {descriptions:?}"
        );
        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn comment_crud() {
        let db = fresh_db().await;
        let owner = db
            .create_user("commenter", "commenter@example.com", "h")
            .await
            .unwrap();
        let repo = db
            .create_repository(owner.id, "commenter", "c-repo", "desc", false, "main")
            .await
            .unwrap();

        // Issue comment lifecycle
        let created_issue = db
            .create_issue(repo.id, "Bug", "details", owner.id, &owner.username)
            .await
            .unwrap();
        // create_issue returns id: 0 (no RETURNING) — fetch the real row.
        let issue = db
            .get_issue(repo.id, created_issue.number)
            .await
            .unwrap()
            .expect("issue exists");
        let c1 = db
            .create_issue_comment(repo.id, issue.id, owner.id, &owner.username, "first!")
            .await
            .unwrap();
        assert!(c1.id > 0);
        assert_eq!(c1.issue_id, Some(issue.id));
        assert_eq!(c1.pr_id, None);
        assert_eq!(c1.body, "first!");

        let c2 = db
            .create_issue_comment(repo.id, issue.id, owner.id, &owner.username, "second")
            .await
            .unwrap();
        let list = db.list_issue_comments(issue.id, 10).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].body, "first!");
        assert_eq!(list[1].body, "second");

        // PR comment lifecycle
        let created_pr = db
            .create_pull_request(repo.id, "PR", "", "main", "main", owner.id, &owner.username, "a", "b")
            .await
            .unwrap();
        // Same as issues: create_pull_request returns id: 0; fetch the real row.
        let pr = db
            .get_pull_request(repo.id, created_pr.number)
            .await
            .unwrap()
            .expect("pr exists");
        let pc = db
            .create_pr_comment(repo.id, pr.id, owner.id, &owner.username, "pr comment")
            .await
            .unwrap();
        assert_eq!(pc.pr_id, Some(pr.id));
        assert_eq!(pc.issue_id, None);
        let pr_list = db.list_pr_comments(pr.id, 10).await.unwrap();
        assert_eq!(pr_list.len(), 1);
        assert_eq!(pr_list[0].body, "pr comment");

        // Delete comment
        assert!(db.delete_comment(c1.id).await.unwrap());
        assert_eq!(db.get_comment(c1.id).await.unwrap(), None);
        let remaining = db.list_issue_comments(issue.id, 10).await.unwrap();
        assert_eq!(remaining.len(), 1);

        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn all_tables_exist_after_migration() {
        let db = fresh_db().await;
        let expected: &[&str] = &[
            "users",
            "repositories",
            "commits",
            "pull_requests",
            "issues",
            "events",
            "access_tokens",
            "groups",
            "group_members",
            "pipelines",
            "pipeline_runs",
            "jobs",
            "logs",
            "comments",
            "stars",
            "webhooks",
            "password_reset_tokens",
        ];
        for table in expected {
            let row: (i64,) = sqlx::query_as(&format!(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = current_schema() AND table_name = $1"
            ))
            .bind(table)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(row.0, 1, "table {table} should exist");
        }
        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn list_events_filter_combinations() {
        // Cover all four branches in list_events: (None, None),
        // (Some(rid), None), (None, Some(et)), (Some(rid), Some(et)).
        let db = fresh_db().await;
        for (i, et) in [
            EventType::Push,
            EventType::Push,
            EventType::IssueCreated,
            EventType::Push,
        ]
        .iter()
        .enumerate()
        {
            let _ = db
                .insert_event(&Event {
                    id: 0,
                    event_type: *et,
                    repo_id: if i % 2 == 0 { Some(i as i64) } else { None },
                    actor_id: Some(1),
                    payload: serde_json::json!({"i": i}),
                    created_at: Utc::now(),
                })
                .await
                .unwrap();
        }

        // (None, None) — all four events
        let all = db.list_events(None, None, 50).await.unwrap();
        assert_eq!(all.len(), 4);

        // (Some(rid=0), None) — only i=0 has repo_id=0 (i=2 has repo_id=2)
        let r0 = db.list_events(Some(0), None, 50).await.unwrap();
        assert!(r0.iter().all(|e| e.repo_id == Some(0)));
        assert_eq!(r0.len(), 1);

        // (None, Some("push")) — events with type=Push (3 out of 4)
        let pushes = db.list_events(None, Some("push"), 50).await.unwrap();
        assert_eq!(pushes.len(), 3);

        // (Some(rid=2), Some("push")) — events with repo_id=2 AND type=Push
        // (i=2 is IssueCreated so this is empty)
        let r2_pushes = db.list_events(Some(2), Some("push"), 50).await.unwrap();
        assert_eq!(r2_pushes.len(), 0);
        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn list_repositories_filter_by_owner() {
        let db = fresh_db().await;
        let alice = db
            .create_user("alice2", "alice2@example.com", "h")
            .await
            .unwrap();
        let bob = db
            .create_user("bob2", "bob2@example.com", "h")
            .await
            .unwrap();
        db.create_repository(alice.id, "alice2", "alpha", "", false, "main")
            .await
            .unwrap();
        db.create_repository(bob.id, "bob2", "beta", "", false, "main")
            .await
            .unwrap();
        db.create_repository(alice.id, "alice2", "gamma", "", false, "main")
            .await
            .unwrap();

        let alice_repos = db
            .list_repositories(Some("alice2"), 50)
            .await
            .unwrap();
        assert_eq!(alice_repos.len(), 2);
        for r in &alice_repos {
            assert_eq!(r.owner_name, "alice2");
        }

        let all = db.list_repositories(None, 50).await.unwrap();
        assert_eq!(all.len(), 3);
        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn pipeline_crud_and_claim() {
        let db = fresh_db().await;
        let user = db.create_user("cici", "cici@e.io", "h").await.unwrap();
        let repo = db
            .create_repository(user.id, "cici", "cirepo", "d", false, "main")
            .await
            .unwrap();

        // upsert_pipeline creates or refreshes.
        let pid = db
            .upsert_pipeline(repo.id, ".gita-ci.yml", "refs/heads/main", "abc123", "stages: [b]")
            .await
            .unwrap();
        assert!(pid > 0);
        let fetched = db.get_pipeline_by_path(repo.id, ".gita-ci.yml").await.unwrap();
        assert!(fetched.is_some());
        let p = fetched.unwrap();
        assert_eq!(p.ref_sha, "abc123");

        // Calling upsert again with the same path updates ref_sha + content
        // and returns the same id.
        let pid2 = db
            .upsert_pipeline(repo.id, ".gita-ci.yml", "refs/heads/main", "def456", "stages: [b, t]")
            .await
            .unwrap();
        assert_eq!(pid, pid2);
        let p2 = db.get_pipeline(pid).await.unwrap().unwrap();
        assert_eq!(p2.ref_sha, "def456");

        // Insert a run, claim it, and assert state transitions.
        let run_id = db
            .insert_pipeline_run(pid, repo.id, "refs/heads/main", "def456", Some(user.id), None)
            .await
            .unwrap();
        assert!(run_id > 0);

        let claimed = db.claim_pipeline_run("runner-1", 300).await.unwrap().unwrap();
        assert_eq!(claimed.id, run_id);
        assert_eq!(claimed.status, "running");
        assert_eq!(claimed.claimed_by_runner_id.as_deref(), Some("runner-1"));
        assert!(claimed.claimed_until.is_some());

        // A second claim while the lease is held returns None.
        let none = db.claim_pipeline_run("runner-2", 300).await.unwrap();
        assert!(none.is_none());

        // Insert a job and a log line.
        let job_id = db
            .insert_job(run_id, "build", "build", &[], &["echo hi".to_string()])
            .await
            .unwrap();
        let log_id = db
            .insert_log(job_id, 0, "stdout", "hi", "2026-10-19T00:00:00Z")
            .await
            .unwrap();
        assert!(log_id > 0);
        let logs = db.list_logs_since(job_id, -1, 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].line, "hi");

        db.update_pipeline_run_status(run_id, "success").await.unwrap();
        let finished = db.get_pipeline_run(run_id).await.unwrap().unwrap();
        assert_eq!(finished.status, "success");
        assert!(finished.finished_at.is_some());
        assert!(finished.claimed_until.is_none());

        drop_test_db(&db).await;
    }
}
