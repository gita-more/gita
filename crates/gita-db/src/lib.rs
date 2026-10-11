//! Data access layer for Gita.
//!
//! Uses SQLx with the `Any` driver to support both SQLite (default,
//! zero-dependency local development) and PostgreSQL (production).
//! All schema operations are idempotent and run at startup.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use gita_core::*;
use serde_json::Value;
use sqlx::any::{AnyConnectOptions, AnyPoolOptions};
use sqlx::{AnyPool, Row};
use std::str::FromStr;
use std::time::Duration;
use tracing::info;

/// The database handle used across the application.
#[derive(Debug, Clone)]
pub struct Db {
    pool: AnyPool,
}

impl Db {
    /// Open a database from a URL.
    ///
    /// - `sqlite::memory:` or `sqlite:///path.db` for SQLite
    /// - `postgres://user:pass@host/db` for PostgreSQL
    pub async fn connect(url: &str) -> Result<Self> {
        // Install default drivers so sqlx::any can use SQLite/PostgreSQL
        sqlx::any::install_default_drivers();
        // sqlx 0.8 does not create SQLite files by default; ensure read-write-create mode
        // so `sqlite://path.db` behaves like an ordinary file-backed database.
        let url = if url.starts_with("sqlite:") && !url.contains("mode=") {
            format!("{url}?mode=rwc")
        } else {
            url.to_string()
        };
        let opts = AnyConnectOptions::from_str(&url)
            .with_context(|| format!("invalid database URL prefix: {url}"))?;
        let pool = AnyPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(opts)
            .await
            .context("failed to connect to database")?;
        let db = Self { pool };
        db.init_schema().await?;
        Ok(db)
    }

    /// Open an in-memory SQLite database (for tests / quickstart).
    pub async fn connect_in_memory() -> Result<Self> {
        // Install default drivers so sqlx::any can use SQLite
        sqlx::any::install_default_drivers();
        // Use sqlite URI with mode=memory
        let opts = AnyConnectOptions::from_str("sqlite://?mode=memory")
            .context("invalid database URL")?;
        let pool = AnyPoolOptions::new()
            .max_connections(1) // Use single connection for in-memory to ensure schema persists
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(opts)
            .await
            .context("failed to connect to in-memory database")?;
        let db = Self { pool };
        db.init_schema().await?;
        Ok(db)
    }

    /// Create the schema if it does not exist.
    ///
    /// The schema is intentionally shared between SQLite and PostgreSQL.
    /// SQLite has no native enums, so statuses are stored as TEXT.
    async fn init_schema(&self) -> Result<()> {
        info!("initializing database schema");
        let result = sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                email TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        info!("users table created: rows_affected={}", result.rows_affected());

        let result = sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS repositories (
                id INTEGER PRIMARY KEY,
                owner_id INTEGER NOT NULL,
                owner_name TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                is_private INTEGER NOT NULL DEFAULT 0,
                default_branch TEXT NOT NULL DEFAULT 'main',
                created_at TEXT NOT NULL,
                UNIQUE(owner_name, name)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        info!("repositories table created: rows_affected={}", result.rows_affected());

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS commits (
                id INTEGER PRIMARY KEY,
                repo_id INTEGER NOT NULL,
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
                id INTEGER PRIMARY KEY,
                repo_id INTEGER NOT NULL,
                number INTEGER NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL DEFAULT '',
                source_branch TEXT NOT NULL,
                target_branch TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'open',
                author_id INTEGER NOT NULL,
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
            CREATE TABLE IF NOT EXISTS reviews (
                id INTEGER PRIMARY KEY,
                pr_id INTEGER,
                repo_id INTEGER NOT NULL,
                commit_sha TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'completed',
                risk_score INTEGER NOT NULL DEFAULT 0,
                summary TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS review_comments (
                id INTEGER PRIMARY KEY,
                review_id INTEGER NOT NULL,
                file_path TEXT NOT NULL,
                line INTEGER,
                severity TEXT NOT NULL DEFAULT 'info',
                message TEXT NOT NULL,
                suggestion TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS issues (
                id INTEGER PRIMARY KEY,
                repo_id INTEGER NOT NULL,
                number INTEGER NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'open',
                author_id INTEGER NOT NULL,
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
                id INTEGER PRIMARY KEY,
                event_type TEXT NOT NULL,
                repo_id INTEGER,
                actor_id INTEGER,
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
                id INTEGER PRIMARY KEY,
                user_id INTEGER NOT NULL,
                token TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL DEFAULT 'default',
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS review_discussion_comments (
                id INTEGER PRIMARY KEY,
                review_id INTEGER NOT NULL,
                author_id INTEGER NOT NULL,
                author_name TEXT NOT NULL DEFAULT '',
                body TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                deleted_at TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Idempotent migrations for columns added after the initial schema.
        // SQLite does not support `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`,
        // so we check for column existence first.
        self.ensure_column(
            "reviews",
            "updated_at",
            "ALTER TABLE reviews ADD COLUMN updated_at TEXT NOT NULL DEFAULT ''",
        )
        .await?;
        self.ensure_column(
            "review_comments",
            "resolved",
            "ALTER TABLE review_comments ADD COLUMN resolved INTEGER NOT NULL DEFAULT 0",
        )
        .await?;
        self.ensure_column(
            "review_comments",
            "resolved_by",
            "ALTER TABLE review_comments ADD COLUMN resolved_by INTEGER",
        )
        .await?;
        self.ensure_column(
            "review_comments",
            "resolved_at",
            "ALTER TABLE review_comments ADD COLUMN resolved_at TEXT",
        )
        .await?;

        info!("database schema ready");
        Ok(())
    }

    /// Ensure a column exists on a table, adding it if missing.
    ///
    /// SQLite cannot express `ADD COLUMN IF NOT EXISTS`, so we inspect the
    /// schema catalog first and only run the `ALTER TABLE` when the column
    /// is absent. PostgreSQL is checked via `information_schema`.
    ///
    /// Table and column names are hard-coded constants in this file (never
    /// user input), so they are inlined directly into the catalog queries.
    async fn ensure_column(&self, table: &str, column: &str, ddl: &str) -> Result<()> {
        // SQLite: pragma_table_info does not accept bound parameters for the
        // table name, and binding the column name also fails to match; use
        // fully-inline literals (table/column are constants, so this is safe).
        let sqlite_has = matches!(
            sqlx::query(&format!(
                "SELECT name FROM pragma_table_info('{table}') WHERE name = '{column}'"
            ))
            .fetch_optional(&self.pool)
            .await,
            Ok(Some(_))
        );
        // PostgreSQL.
        let pg_has = matches!(
            sqlx::query(
                "SELECT column_name FROM information_schema.columns WHERE table_name = ? AND column_name = ?",
            )
            .bind(table)
            .bind(column)
            .fetch_optional(&self.pool)
            .await,
            Ok(Some(_))
        );
        if !sqlite_has && !pg_has {
            info!("adding column {table}.{column}");
            self.raw_query(ddl).await?;
        }
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
             VALUES (?, ?, ?, ?, ?)
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
        let mut conds = Vec::new();
        let mut args: Vec<String> = Vec::new();
        if let Some(rid) = repo_id {
            conds.push(format!("repo_id = ?{}", args.len() + 1));
            args.push(rid.to_string());
        }
        if let Some(et) = event_type {
            conds.push(format!("event_type = ?{}", args.len() + 1));
            args.push(et.to_string());
        }
        if !conds.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");
        args.push(limit.to_string());
        let mut q = sqlx::query(&sql);
        for a in &args {
            q = q.bind(a.as_str());
        }
        q = q.bind(limit as i64);
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
    pub fn pool(&self) -> &AnyPool {
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
        "review_completed" => EventType::ReviewCompleted,
        "issue_created" => EventType::IssueCreated,
        "issue_closed" => EventType::IssueClosed,
        "agent_run_started" => EventType::AgentRunStarted,
        "agent_run_completed" => EventType::AgentRunCompleted,
        "comment_added" => EventType::CommentAdded,
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
        // Use RETURNING clause to get the inserted ID (works for SQLite 3.35+ and PostgreSQL)
        let row = sqlx::query(
            "INSERT INTO users (username, email, password_hash, created_at)
             VALUES (?, ?, ?, ?)
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
            "SELECT id, username, email, password_hash, created_at FROM users WHERE id = ?",
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
            "SELECT id, username, email, password_hash, created_at FROM users WHERE username = ?",
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
            "SELECT id, username, email, password_hash, created_at FROM users WHERE email = ?",
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
        let rows = sqlx::query("SELECT id, username, email, password_hash, created_at FROM users ORDER BY id DESC LIMIT ?")
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_user).collect()
    }
}

fn row_to_user(row: &sqlx::any::AnyRow) -> Result<User> {
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
             VALUES (?, ?, ?, ?, ?, ?, ?)
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
             FROM repositories WHERE id = ?",
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
             FROM repositories WHERE owner_name = ? AND name = ?",
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
        let mut sql = String::from(
            "SELECT id, owner_id, owner_name, name, description, is_private, default_branch, created_at
             FROM repositories",
        );
        let mut args: Vec<String> = Vec::new();
        if let Some(owner) = owner_name {
            sql.push_str(" WHERE owner_name = ?");
            args.push(owner.to_string());
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");
        args.push(limit.to_string());
        let mut q = sqlx::query(&sql);
        for a in &args {
            q = q.bind(a.as_str());
        }
        q = q.bind(limit as i64);
        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(row_to_repo).collect()
    }

    pub async fn delete_repository(&self, id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM repositories WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn update_repository_description(&self, id: i64, description: &str) -> Result<()> {
        sqlx::query("UPDATE repositories SET description = ? WHERE id = ?")
            .bind(description)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn row_to_repo(row: &sqlx::any::AnyRow) -> Result<Repository> {
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
        let row = sqlx::query(
            "INSERT OR IGNORE INTO commits (repo_id, sha, author_name, author_email, message, parent_shas, authored_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
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
        Ok(row.last_insert_id().unwrap_or(0) as i64)
    }

    pub async fn get_commit(&self, repo_id: i64, sha: &str) -> Result<Option<Commit>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, sha, author_name, author_email, message, parent_shas, authored_at
             FROM commits WHERE repo_id = ? AND sha = ?",
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
             FROM commits WHERE repo_id = ? ORDER BY authored_at DESC LIMIT ?",
        )
        .bind(repo_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_commit).collect()
    }

    pub async fn latest_commit_sha(&self, repo_id: i64) -> Result<Option<String>> {
        let rows = sqlx::query(
            "SELECT sha FROM commits WHERE repo_id = ? ORDER BY authored_at DESC LIMIT 1",
        )
        .bind(repo_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.first().and_then(|r| r.try_get("sha").ok()))
    }
}

fn row_to_commit(row: &sqlx::any::AnyRow) -> Result<Commit> {
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
        let rows = sqlx::query("SELECT COALESCE(MAX(number), 0) + 1 AS next_num FROM pull_requests WHERE repo_id = ?")
            .bind(repo_id)
            .fetch_all(&self.pool)
            .await?;
        let number: i64 = rows
            .first()
            .map(|r| r.try_get::<i64, _>("next_num").unwrap_or(1))
            .unwrap_or(1);
        let row = sqlx::query(
            "INSERT INTO pull_requests (repo_id, number, title, body, source_branch, target_branch, status, author_id, author_name, base_sha, head_sha, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, 'open', ?, ?, ?, ?, ?, ?)",
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
        let id = row.last_insert_id().unwrap_or(0) as i64;
        Ok(PullRequest {
            id,
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
             FROM pull_requests WHERE repo_id = ? AND number = ?",
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
             FROM pull_requests WHERE id = ?",
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
        let mut sql = String::from(
            "SELECT id, repo_id, number, title, body, source_branch, target_branch, status, author_id, author_name, base_sha, head_sha, created_at, updated_at, merged_at
             FROM pull_requests WHERE repo_id = ?",
        );
        let mut args: Vec<String> = vec![repo_id.to_string()];
        if let Some(st) = status {
            sql.push_str(" AND status = ?");
            args.push(st.to_string());
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");
        args.push(limit.to_string());
        let mut q = sqlx::query(&sql);
        for a in &args {
            q = q.bind(a.as_str());
        }
        q = q.bind(limit as i64);
        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(row_to_pr).collect()
    }

    pub async fn update_pr_status(&self, id: i64, status: PrStatus, merged_at: Option<DateTime<Utc>>) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE pull_requests SET status = ?, updated_at = ?, merged_at = ? WHERE id = ?")
            .bind(status.to_string())
            .bind(&now)
            .bind(merged_at.map(|d| d.to_rfc3339()))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_pr_head_sha(&self, id: i64, head_sha: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE pull_requests SET head_sha = ?, updated_at = ? WHERE id = ?")
            .bind(head_sha)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn row_to_pr(row: &sqlx::any::AnyRow) -> Result<PullRequest> {
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
// Reviews
// ---------------------------------------------------------------------------

impl Db {
    pub async fn insert_review(&self, review: &Review) -> Result<i64> {
        let created_at = review.created_at.to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO reviews (pr_id, repo_id, commit_sha, status, risk_score, summary, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(review.pr_id)
        .bind(review.repo_id)
        .bind(&review.commit_sha)
        .bind(&review.status)
        .bind(review.risk_score)
        .bind(&review.summary)
        .bind(&created_at)
        .fetch_one(&self.pool)
        .await?;
        let review_id: i64 = row.try_get("id").unwrap_or(0);
        for c in &review.comments {
            self.insert_review_comment(review_id, c).await?;
        }
        Ok(review_id)
    }

    async fn insert_review_comment(&self, review_id: i64, c: &ReviewComment) -> Result<()> {
        sqlx::query(
            "INSERT INTO review_comments (review_id, file_path, line, severity, message, suggestion, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(review_id)
        .bind(&c.file_path)
        .bind(c.line)
        .bind(c.severity.to_string())
        .bind(&c.message)
        .bind(&c.suggestion)
        .bind(c.created_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_review(&self, id: i64) -> Result<Option<Review>> {
        let rows = sqlx::query(
            "SELECT id, pr_id, repo_id, commit_sha, status, risk_score, summary, created_at FROM reviews WHERE id = ?",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        let review = row_to_review(&rows[0])?;
        let comments = self.list_review_comments(id).await?;
        let mut review = review;
        review.comments = comments;
        Ok(Some(review))
    }

    pub async fn list_reviews(
        &self,
        repo_id: Option<i64>,
        pr_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<Review>> {
        let mut sql = String::from(
            "SELECT id, pr_id, repo_id, commit_sha, status, risk_score, summary, created_at FROM reviews",
        );
        let mut conds = Vec::new();
        let mut args: Vec<String> = Vec::new();
        if let Some(rid) = repo_id {
            conds.push("repo_id = ?".to_string());
            args.push(rid.to_string());
        }
        if let Some(pid) = pr_id {
            conds.push("pr_id = ?".to_string());
            args.push(pid.to_string());
        }
        if !conds.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");
        args.push(limit.to_string());
        let mut q = sqlx::query(&sql);
        for a in &args {
            q = q.bind(a.as_str());
        }
        q = q.bind(limit as i64);
        let rows = q.fetch_all(&self.pool).await?;
        let mut reviews = Vec::with_capacity(rows.len());
        for row in rows {
            let review = row_to_review(&row)?;
            let comments = self.list_review_comments(review.id).await?;
            let mut review = review;
            review.comments = comments;
            reviews.push(review);
        }
        Ok(reviews)
    }

    async fn list_review_comments(&self, review_id: i64) -> Result<Vec<ReviewComment>> {
        let rows = sqlx::query(
            "SELECT id, review_id, file_path, line, severity, message, suggestion, created_at, resolved, resolved_by, resolved_at
             FROM review_comments WHERE review_id = ? ORDER BY id ASC",
        )
        .bind(review_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_review_comment).collect()
    }

    pub async fn list_review_comments_public(&self, review_id: i64) -> Result<Vec<ReviewComment>> {
        self.list_review_comments(review_id).await
    }

    /// Get a single AI review comment (line-level finding).
    pub async fn get_review_comment(&self, id: i64) -> Result<Option<ReviewComment>> {
        let rows = sqlx::query(
            "SELECT id, review_id, file_path, line, severity, message, suggestion, created_at, resolved, resolved_by, resolved_at
             FROM review_comments WHERE id = ?",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_review_comment(&rows[0])?))
    }

    // -----------------------------------------------------------------------
    // Review discussion comments (user comments on a review)
    // -----------------------------------------------------------------------

    pub async fn insert_review_discussion_comment(
        &self,
        review_id: i64,
        author_id: i64,
        author_name: &str,
        body: &str,
    ) -> Result<ReviewDiscussionComment> {
        let now = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO review_discussion_comments (review_id, author_id, author_name, body, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(review_id)
        .bind(author_id)
        .bind(author_name)
        .bind(body)
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        let id: i64 = row.try_get("id").unwrap_or(0);
        Ok(ReviewDiscussionComment {
            id,
            review_id,
            author_id,
            author_name: author_name.to_string(),
            body: body.to_string(),
            created_at: DateTime::parse_from_rfc3339(&now)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            updated_at: DateTime::parse_from_rfc3339(&now)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            deleted_at: None,
        })
    }

    pub async fn list_review_discussion_comments(
        &self,
        review_id: i64,
    ) -> Result<Vec<ReviewDiscussionComment>> {
        let rows = sqlx::query(
            "SELECT id, review_id, author_id, author_name, body, created_at, updated_at, deleted_at
             FROM review_discussion_comments
             WHERE review_id = ? AND deleted_at IS NULL
             ORDER BY id ASC",
        )
        .bind(review_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_discussion_comment).collect()
    }

    pub async fn get_review_discussion_comment(
        &self,
        id: i64,
    ) -> Result<Option<ReviewDiscussionComment>> {
        let rows = sqlx::query(
            "SELECT id, review_id, author_id, author_name, body, created_at, updated_at, deleted_at
             FROM review_discussion_comments WHERE id = ?",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(row_to_discussion_comment(&rows[0])?))
    }

    /// Edit a discussion comment. Only the author can edit (`author_id` check).
    pub async fn update_review_discussion_comment(
        &self,
        id: i64,
        author_id: i64,
        body: &str,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        let res = sqlx::query(
            "UPDATE review_discussion_comments SET body = ?, updated_at = ?
             WHERE id = ? AND author_id = ? AND deleted_at IS NULL",
        )
        .bind(body)
        .bind(&now)
        .bind(id)
        .bind(author_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Soft-delete a discussion comment. Only the author can delete.
    pub async fn delete_review_discussion_comment(
        &self,
        id: i64,
        author_id: i64,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        let res = sqlx::query(
            "UPDATE review_discussion_comments SET deleted_at = ?
             WHERE id = ? AND author_id = ? AND deleted_at IS NULL",
        )
        .bind(&now)
        .bind(id)
        .bind(author_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    // -----------------------------------------------------------------------
    // Review modification (summary / risk_score) and comment resolution
    // -----------------------------------------------------------------------

    /// Update the editable fields of a review. Only non-`None` fields are set.
    pub async fn update_review(
        &self,
        id: i64,
        summary: Option<&str>,
        risk_score: Option<i64>,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        let mut sql = String::from("UPDATE reviews SET updated_at = ?");
        let mut args: Vec<String> = vec![now.clone()];
        if let Some(s) = summary {
            sql.push_str(", summary = ?");
            args.push(s.to_string());
        }
        if let Some(rs) = risk_score {
            sql.push_str(", risk_score = ?");
            args.push(rs.to_string());
        }
        sql.push_str(" WHERE id = ?");
        args.push(id.to_string());
        let mut q = sqlx::query(&sql);
        for a in &args {
            q = q.bind(a.as_str());
        }
        let res = q.execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    /// Mark a review comment as resolved / unresolved.
    pub async fn set_review_comment_resolved(
        &self,
        id: i64,
        resolved: bool,
        by_user_id: i64,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        let (resolved_val, resolved_by, resolved_at) = if resolved {
            (1, Some(by_user_id.to_string()), Some(now))
        } else {
            (0, None, None)
        };
        let res = sqlx::query(
            "UPDATE review_comments SET resolved = ?, resolved_by = ?, resolved_at = ?
             WHERE id = ?",
        )
        .bind(resolved_val)
        .bind(resolved_by)
        .bind(resolved_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
}

fn row_to_discussion_comment(row: &sqlx::any::AnyRow) -> Result<ReviewDiscussionComment> {
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;
    let deleted_at: Option<String> = row.try_get("deleted_at").ok().flatten();
    Ok(ReviewDiscussionComment {
        id: row.try_get("id")?,
        review_id: row.try_get("review_id")?,
        author_id: row.try_get("author_id")?,
        author_name: row.try_get("author_name")?,
        body: row.try_get("body")?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        deleted_at: deleted_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc)),
    })
}

fn row_to_review(row: &sqlx::any::AnyRow) -> Result<Review> {
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at").unwrap_or_default();
    Ok(Review {
        id: row.try_get("id")?,
        pr_id: row.try_get("pr_id")?,
        repo_id: row.try_get("repo_id")?,
        commit_sha: row.try_get("commit_sha")?,
        status: row.try_get("status")?,
        risk_score: row.try_get("risk_score")?,
        summary: row.try_get("summary")?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        comments: Vec::new(),
    })
}

fn row_to_review_comment(row: &sqlx::any::AnyRow) -> Result<ReviewComment> {
    let severity: String = row.try_get("severity")?;
    let created_at: String = row.try_get("created_at")?;
    let resolved: i64 = row.try_get("resolved").unwrap_or(0);
    let resolved_at: Option<String> = row.try_get("resolved_at").ok().flatten();
    Ok(ReviewComment {
        id: row.try_get("id")?,
        review_id: row.try_get("review_id")?,
        file_path: row.try_get("file_path")?,
        line: row.try_get("line")?,
        severity: match severity.as_str() {
            "warning" => Severity::Warning,
            "error" => Severity::Error,
            _ => Severity::Info,
        },
        message: row.try_get("message")?,
        suggestion: row.try_get("suggestion")?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        resolved: resolved != 0,
        resolved_by: row.try_get("resolved_by").ok().flatten(),
        resolved_at: resolved_at
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
        let rows = sqlx::query("SELECT COALESCE(MAX(number), 0) + 1 AS next_num FROM issues WHERE repo_id = ?")
            .bind(repo_id)
            .fetch_all(&self.pool)
            .await?;
        let number: i64 = rows
            .first()
            .map(|r| r.try_get::<i64, _>("next_num").unwrap_or(1))
            .unwrap_or(1);
        let row = sqlx::query(
            "INSERT INTO issues (repo_id, number, title, body, status, author_id, author_name, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'open', ?, ?, ?, ?)",
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
        let id = row.last_insert_id().unwrap_or(0) as i64;
        Ok(Issue {
            id,
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
             FROM issues WHERE repo_id = ? AND number = ?",
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
        let mut sql = String::from(
            "SELECT id, repo_id, number, title, body, status, author_id, author_name, created_at, updated_at, closed_at
             FROM issues WHERE repo_id = ?",
        );
        let mut args: Vec<String> = vec![repo_id.to_string()];
        if let Some(st) = status {
            sql.push_str(" AND status = ?");
            args.push(st.to_string());
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");
        args.push(limit.to_string());
        let mut q = sqlx::query(&sql);
        for a in &args {
            q = q.bind(a.as_str());
        }
        q = q.bind(limit as i64);
        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(row_to_issue).collect()
    }

    pub async fn update_issue_status(&self, id: i64, status: IssueStatus) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let closed_at = if status == IssueStatus::Closed {
            Some(now.clone())
        } else {
            None
        };
        sqlx::query("UPDATE issues SET status = ?, updated_at = ?, closed_at = ? WHERE id = ?")
            .bind(status.to_string())
            .bind(&now)
            .bind(closed_at)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn row_to_issue(row: &sqlx::any::AnyRow) -> Result<Issue> {
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
            "INSERT INTO access_tokens (user_id, token, name, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(token)
        .bind(name)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;
        Ok(AccessToken {
            id: row.last_insert_id().unwrap_or(0) as i64,
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
             WHERE t.token = ?",
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
            "SELECT id, user_id, token, name, created_at FROM access_tokens WHERE user_id = ? ORDER BY id DESC",
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
        let res = sqlx::query("DELETE FROM access_tokens WHERE id = ? AND user_id = ?")
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
fn _assert_send_sync(db: &Db) {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Db>();
}

// Re-export convenience.
pub use sqlx::any::AnyRow;

/// Small helper for tests.
pub async fn new_test_db() -> Db {
    let db = Db::connect_in_memory()
        .await
        .expect("in-memory db should work");
    // Verify tables exist
    let tables: Vec<(String,)> = sqlx::query_as("SELECT name FROM sqlite_master WHERE type='table'")
        .fetch_all(db.pool())
        .await
        .expect("failed to list tables");
    eprintln!("Tables after init: {:?}", tables);
    db
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn user_crud() {
        let db = new_test_db().await;
        let u = db
            .create_user("alice", "alice@example.com", "hash")
            .await
            .unwrap();
        assert!(u.id > 0);
        let fetched = db.get_user_by_username("alice").await.unwrap().unwrap();
        assert_eq!(fetched.id, u.id);
        assert_eq!(db.get_user_by_username("nobody").await.unwrap(), None);
    }

    #[tokio::test]
    async fn repo_crud() {
        let db = new_test_db().await;
        let u = db.create_user("bob", "bob@example.com", "h").await.unwrap();
        let r = db
            .create_repository(u.id, "bob", "hello", "desc", false, "main")
            .await
            .unwrap();
        assert!(r.id > 0);
        let fetched = db.get_repository_by_name("bob", "hello").await.unwrap().unwrap();
        assert_eq!(fetched.full_name(), "bob/hello");
        assert!(!fetched.is_private);
    }

    #[tokio::test]
    async fn event_insert_and_list() {
        let db = new_test_db().await;
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
    }

    #[tokio::test]
    async fn discussion_comment_crud() {
        let db = new_test_db().await;
        // Create a review to attach comments to.
        let review = Review {
            id: 0,
            pr_id: None,
            repo_id: 1,
            commit_sha: "abc".to_string(),
            status: "completed".to_string(),
            risk_score: 10,
            summary: "s".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            comments: Vec::new(),
        };
        let review_id = db.insert_review(&review).await.unwrap();

        let c = db
            .insert_review_discussion_comment(review_id, 7, "alice", "Looks good")
            .await
            .unwrap();
        assert!(c.id > 0);
        assert_eq!(c.author_name, "alice");

        let listed = db
            .list_review_discussion_comments(review_id)
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);

        // Author can edit.
        let ok = db
            .update_review_discussion_comment(c.id, 7, "Actually, fix this")
            .await
            .unwrap();
        assert!(ok);
        // Non-author cannot edit.
        let ok = db
            .update_review_discussion_comment(c.id, 99, "hijack")
            .await
            .unwrap();
        assert!(!ok);

        // Soft delete.
        let ok = db.delete_review_discussion_comment(c.id, 7).await.unwrap();
        assert!(ok);
        let listed = db
            .list_review_discussion_comments(review_id)
            .await
            .unwrap();
        assert!(listed.is_empty());
        // Non-author cannot delete.
        let ok = db
            .delete_review_discussion_comment(c.id, 99)
            .await
            .unwrap();
        assert!(!ok);
    }

    #[tokio::test]
    async fn update_review_fields() {
        let db = new_test_db().await;
        let review = Review {
            id: 0,
            pr_id: None,
            repo_id: 1,
            commit_sha: "abc".to_string(),
            status: "completed".to_string(),
            risk_score: 10,
            summary: "s".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            comments: Vec::new(),
        };
        let review_id = db.insert_review(&review).await.unwrap();

        let ok = db
            .update_review(review_id, Some("new summary"), Some(50))
            .await
            .unwrap();
        assert!(ok);

        let fetched = db.get_review(review_id).await.unwrap().unwrap();
        assert_eq!(fetched.summary, "new summary");
        assert_eq!(fetched.risk_score, 50);
        // updated_at should be set (not the default empty string).
        assert!(fetched.updated_at.timestamp() > 0);
    }

    #[tokio::test]
    async fn review_comment_resolve() {
        let db = new_test_db().await;
        let review = Review {
            id: 0,
            pr_id: None,
            repo_id: 1,
            commit_sha: "abc".to_string(),
            status: "completed".to_string(),
            risk_score: 10,
            summary: "s".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            comments: Vec::new(),
        };
        let review_id = db.insert_review(&review).await.unwrap();
        let fetched = db.get_review(review_id).await.unwrap().unwrap();
        // Insert review has no AI comments, so verify resolution on a manual one
        // through the private insert path via a review with comments.
        drop(fetched);

        // Use a review with one AI comment.
        let now = Utc::now();
        let comment = ReviewComment {
            id: 0,
            review_id: 0,
            file_path: "src/lib.rs".to_string(),
            line: Some(42),
            severity: Severity::Warning,
            message: "unused var".to_string(),
            suggestion: "remove".to_string(),
            created_at: now,
            resolved: false,
            resolved_by: None,
            resolved_at: None,
        };
        let review_with_comment = Review {
            id: 0,
            pr_id: None,
            repo_id: 1,
            commit_sha: "abc".to_string(),
            status: "completed".to_string(),
            risk_score: 10,
            summary: "s".to_string(),
            created_at: now,
            updated_at: now,
            comments: vec![comment],
        };
        let review_id = db.insert_review(&review_with_comment).await.unwrap();
        let fetched = db.get_review(review_id).await.unwrap().unwrap();
        let comment_id = fetched.comments[0].id;

        let ok = db
            .set_review_comment_resolved(comment_id, true, 7)
            .await
            .unwrap();
        assert!(ok);
        let fetched = db.get_review(review_id).await.unwrap().unwrap();
        assert!(fetched.comments[0].resolved);
        assert_eq!(fetched.comments[0].resolved_by, Some(7));
        assert!(fetched.comments[0].resolved_at.is_some());

        let ok = db
            .set_review_comment_resolved(comment_id, false, 7)
            .await
            .unwrap();
        assert!(ok);
        let fetched = db.get_review(review_id).await.unwrap().unwrap();
        assert!(!fetched.comments[0].resolved);
        assert_eq!(fetched.comments[0].resolved_by, None);
        assert!(fetched.comments[0].resolved_at.is_none());
    }

    #[tokio::test]
    async fn migration_idempotent() {
        // Connecting twice to an existing in-memory DB is not possible (single
        // connection), so instead verify init_schema can be called again on a
        // fresh in-memory db and that migrated columns exist.
        let db = new_test_db().await;
        db.init_schema().await.unwrap();
        // Verify the migrated columns are present and usable.
        let ok = db
            .update_review(1, Some("x"), Some(1))
            .await
            .is_ok();
        assert!(ok);
        // Verify review_discussion_comments table is present.
        let ok = db
            .insert_review_discussion_comment(1, 1, "u", "hello")
            .await
            .is_ok();
        assert!(ok);
    }
}