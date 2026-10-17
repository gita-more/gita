//! Data access layer for Gita.
//!
//! PostgreSQL-only. Schema is created idempotently at startup.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use gita_core::*;
use serde_json::Value;
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::Row;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tracing::info;

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
        // If the URL has a ?options= fragment, libpq already applied it.
        // For tests we additionally pin search_path via the URL; either way
        // init_schema runs against whatever schema the role/user lands in by
        // default — call sites that need a specific schema should use
        // `?options=-c search_path=...` in the URL or connect via a role that
        // has a default search_path.
        let db = Self { pool };
        db.init_schema().await?;
        Ok(db)
    }

    /// Open an existing pool. Useful for tests that share a connection pool
    /// across multiple schema-isolated tests.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create the schema if it does not exist.
    async fn init_schema(&self) -> Result<()> {
        info!("initializing database schema");
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                email TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        info!("users table ready");

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS repositories (
                id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                owner_id BIGINT NOT NULL,
                owner_name TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                is_private BIGINT NOT NULL DEFAULT 0,
                default_branch TEXT NOT NULL DEFAULT 'main',
                created_at TEXT NOT NULL,
                UNIQUE(owner_name, name)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        info!("repositories table ready");

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS commits (
                id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                repo_id BIGINT NOT NULL,
                sha TEXT NOT NULL,
                author_name TEXT NOT NULL,
                author_email TEXT NOT NULL,
                message TEXT NOT NULL,
                parent_shas TEXT NOT NULL DEFAULT '[]',
                authored_at TEXT NOT NULL,
                UNIQUE(repo_id, sha)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS pull_requests (
                id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                repo_id BIGINT NOT NULL,
                number BIGINT NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL DEFAULT '',
                source_branch TEXT NOT NULL,
                target_branch TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'open',
                author_id BIGINT NOT NULL,
                author_name TEXT NOT NULL,
                base_sha TEXT NOT NULL DEFAULT '',
                head_sha TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                merged_at TEXT,
                UNIQUE(repo_id, number)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS issues (
                id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                repo_id BIGINT NOT NULL,
                number BIGINT NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'open',
                author_id BIGINT NOT NULL,
                author_name TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                closed_at TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS events (
                id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                event_type TEXT NOT NULL,
                repo_id BIGINT,
                actor_id BIGINT,
                payload TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS access_tokens (
                id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                user_id BIGINT NOT NULL,
                token TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL DEFAULT 'default',
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        info!("database schema ready");
        Ok(())
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
        "agent_run_started" => EventType::AgentRunStarted,
        "agent_run_completed" => EventType::AgentRunCompleted,
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
        let created_at = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO users (username, email, password_hash, created_at)
             VALUES ($1, $2, $3, $4)
             RETURNING id",
        )
        .bind(username)
        .bind(email)
        .bind(password_hash)
        .bind(&created_at)
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
        })
    }

    pub async fn get_user_by_id(&self, id: i64) -> Result<Option<User>> {
        let rows = sqlx::query(
            "SELECT id, username, email, password_hash, created_at FROM users WHERE id = $1",
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
            "SELECT id, username, email, password_hash, created_at FROM users WHERE username = $1",
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
            "SELECT id, username, email, password_hash, created_at FROM users WHERE email = $1",
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
        let rows = sqlx::query("SELECT id, username, email, password_hash, created_at FROM users ORDER BY id DESC LIMIT $1")
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
    ) -> Result<Vec<Repository>> {
        let rows = if let Some(owner) = owner_name {
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

    pub async fn update_repository_description(&self, id: i64, description: &str) -> Result<()> {
        sqlx::query("UPDATE repositories SET description = $2 WHERE id = $1")
            .bind(id)
            .bind(description)
            .execute(&self.pool)
            .await?;
        Ok(())
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
        sqlx::query(
            "INSERT INTO access_tokens (user_id, token, name, created_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(user_id)
        .bind(token)
        .bind(name)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;
        Ok(AccessToken {
            id: 0, // not returned; re-fetch via get_user_by_token if needed
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
            "SELECT u.id, u.username, u.email, u.password_hash, u.created_at
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
/// `search_path` to it, runs `init_schema`, and returns the Db. The schema is
/// left behind on the server — `drop_test_db` cleans it up after the test.
pub async fn new_test_db(url: &str) -> Db {
    let schema = format!(
        "gita_test_{}_{}",
        std::process::id(),
        TEST_SCHEMA_COUNTER.fetch_add(1, Ordering::SeqCst)
    );

    // Connect directly to the DB and create the schema; we cannot use the
    // Db::connect path because init_schema runs before we can pin search_path.
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .connect(url)
        .await
        .expect("failed to connect to test database");

    // CREATE SCHEMA IF NOT EXISTS, then pin search_path on every connection.
    sqlx::query(&format!("CREATE SCHEMA \"{schema}\""))
        .execute(&pool)
        .await
        .expect("failed to create test schema");
    sqlx::query(&format!("SET search_path TO \"{schema}\""))
        .execute(&pool)
        .await
        .expect("failed to set search_path");

    let db = Db::from_pool(pool);
    db.init_schema()
        .await
        .expect("failed to initialize schema");
    db
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
        let fetched = db.get_user_by_username("alice").await.unwrap().unwrap();
        assert_eq!(fetched.id, u.id);
        assert_eq!(db.get_user_by_username("nobody").await.unwrap(), None);
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
        // Re-running init_schema against an existing schema must succeed
        // (every CREATE TABLE is IF NOT EXISTS).
        db.init_schema().await.unwrap();
        drop_test_db(&db).await;
    }
}
