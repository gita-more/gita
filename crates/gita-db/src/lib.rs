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
}
