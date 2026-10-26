//! Data access layer for Gita.
//!
//! PostgreSQL-only. Schema is created idempotently at startup.
//!
//! The `Db` methods are split across per-domain modules so each domain's
//! queries, row converters, and structures live in one file:
//!
//! | Module        | Domain                                          |
//! |---------------|-------------------------------------------------|
//! | `users`       | accounts, admin flag, locale                     |
//! | `repositories` | repository CRUD, visibility, transfer          |
//! | `commits`     | commit storage and lookup                       |
//! | `pull_requests` | PR CRUD, status transitions                    |
//! | `issues`      | issue CRUD and listing                           |
//! | `comments`   | PR + issue comments                              |
//! | `group_members` | group membership roles                        |
//! | `groups`     | groups, group repositories, cross-repo issues   |
//! | `ssh_keys`     | SSH public keys + fingerprint helpers           |
//! | `password_reset` | reset tokens, atomic claim                     |
//! | `webhooks`     | webhook rows                                   |
//! | `pages`         | GitHub-Pages-style publishing config            |
//! | `stars`         | repository stars                                |
//! | `access_tokens` | personal access tokens, token hashing           |
//! | `pipeline`      | in-repo CI: pipelines, runs, jobs, logs          |

pub mod access;
mod access_tokens;
mod comments;
mod commits;
mod group_members;
mod groups;
mod issues;
mod pages;
mod password_reset;
mod pipeline;
mod pull_requests;
mod repositories;
mod ssh_keys;
mod stars;
mod users;
mod webhooks;

pub use access_tokens::hash_token;
pub use pages::PagesRow;
pub use password_reset::PasswordResetRow;
pub use pipeline::{JobRow, LogRow, PipelineRow, PipelineRunRow};
pub use ssh_keys::{ssh_fingerprint, ssh_key_is_valid, SshKeyRow};
pub use webhooks::WebhookRow;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use gita_core::*;
use serde_json::Value;
use sqlx::postgres::{PgPool, PgPoolOptions};
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
            // Skip events whose type this build does not recognise rather
            // than failing the whole listing.
            let Some(event_type) = parse_event_type(&event_type) else {
                continue;
            };
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

/// Parse an event type string.
///
/// Returns `None` for a type this build does not know about. An unknown
/// value must not fail the whole listing: a newly added event type would
/// otherwise make `GET /api/events` return 500 for every event, not just
/// the unrecognised one.
fn parse_event_type(s: &str) -> Option<EventType> {
    Some(match s {
        "repo_created" => EventType::RepoCreated,
        "repo_deleted" => EventType::RepoDeleted,
        "push" => EventType::Push,
        "pull_request_opened" => EventType::PullRequestOpened,
        "pull_request_merged" => EventType::PullRequestMerged,
        "pull_request_closed" => EventType::PullRequestClosed,
        "issue_created" => EventType::IssueCreated,
        "issue_closed" => EventType::IssueClosed,
        "comment_created" => EventType::CommentCreated,
        "comment_deleted" => EventType::CommentDeleted,
        "user_registered" => EventType::UserRegistered,
        "agent_run_started" => EventType::AgentRunStarted,
        "agent_run_completed" => EventType::AgentRunCompleted,
        "pipeline_requested" => EventType::PipelineRequested,
        "pipeline_started" => EventType::PipelineStarted,
        "pipeline_completed" => EventType::PipelineCompleted,
        "pipeline_failed" => EventType::PipelineFailed,
        "job_started" => EventType::JobStarted,
        "job_completed" => EventType::JobCompleted,
        _ => return None,
    })
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
        env::var("GITA_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://gita:gita@127.0.0.1:55432/gita".to_string())
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
    async fn admin_user_management() {
        let db = fresh_db().await;

        // Created with an explicit role.
        let admin = db
            .create_user_with_role("root", "root@example.com", "h", "zh", true)
            .await
            .unwrap();
        assert!(admin.is_admin);
        let plain = db
            .create_user_with_role("peon", "peon@example.com", "h", "en", false)
            .await
            .unwrap();
        assert!(!plain.is_admin);
        assert_eq!(db.count_admins().await.unwrap(), 1);

        // Granting the role works.
        let promoted = db
            .update_user_admin(plain.id, true)
            .await
            .unwrap()
            .expect("user exists");
        assert!(promoted.is_admin);
        assert_eq!(db.count_admins().await.unwrap(), 2);

        // Demoting one of two admins works.
        let demoted = db
            .update_user_admin(plain.id, false)
            .await
            .unwrap()
            .expect("user exists");
        assert!(!demoted.is_admin);

        // Demoting the very last admin is refused.
        let err = db.update_user_admin(admin.id, false).await.unwrap_err();
        assert!(
            err.to_string().contains("last administrator"),
            "unexpected error: {err}"
        );

        // Ownership helpers and deletion.
        let repo = db
            .create_repository(admin.id, "root", "solo", "", false, "main", None)
            .await
            .unwrap();
        let group = db.create_group("roots", "", admin.id, None).await.unwrap();
        assert_eq!(
            db.list_repositories_owned_by(admin.id).await.unwrap().len(),
            1
        );
        assert_eq!(db.list_groups_owned_by(admin.id).await.unwrap().len(), 1);

        // The repository must go before the group (no cascade on group_id),
        // matching what the admin delete route does.
        assert!(db.delete_repository(repo.id).await.unwrap());
        assert!(db.delete_group(group.id).await.unwrap());
        assert!(db.delete_user(admin.id).await.unwrap());
        assert!(db.get_user_by_username("root").await.unwrap().is_none());
        assert_eq!(db.count_admins().await.unwrap(), 0);
        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn repo_crud() {
        let db = fresh_db().await;
        let u = db.create_user("bob", "bob@example.com", "h").await.unwrap();
        let r = db
            .create_repository(u.id, "bob", "hello", "desc", false, "main", None)
            .await
            .unwrap();
        assert!(r.id > 0);
        let fetched = db
            .get_repository_by_name("bob", "hello")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.full_name(), "bob/hello");
        assert!(!fetched.is_private);
        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn ssh_key_crud() {
        let db = fresh_db().await;
        let u = db
            .create_user("sshuser", "ssh@example.com", "h")
            .await
            .unwrap();
        let pk = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAmSSX13Ub9vIWCGFHUJAd04tQPCJxOTY7Gl1OerO+RE";

        let key = db.add_ssh_key(u.id, "laptop", pk).await.unwrap();
        assert!(key.id > 0);
        assert_eq!(key.user_id, u.id);
        assert_eq!(key.title, "laptop");
        assert_eq!(key.public_key, pk);

        // Listing returns the new key (ordered by id DESC).
        let keys = db.list_ssh_keys(u.id).await.unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].id, key.id);
        assert_eq!(keys[0].title, "laptop");

        // Another user cannot see or delete this key.
        let other = db
            .create_user("sshother", "ssh2@example.com", "h")
            .await
            .unwrap();
        assert!(db.list_ssh_keys(other.id).await.unwrap().is_empty());
        assert!(!db.delete_ssh_key(key.id, other.id).await.unwrap());

        // Owner can delete.
        assert!(db.delete_ssh_key(key.id, u.id).await.unwrap());
        assert!(db.list_ssh_keys(u.id).await.unwrap().is_empty());
        drop_test_db(&db).await;
    }

    /// Registering the same public key twice must not create a second
    /// row. The `ssh_keys` table has a unique index on
    /// `(user_id, public_key)`; without it the key list accumulated
    /// duplicates and every SSH authentication — which scans the whole
    /// table recomputing a SHA-256 per row — got slower.
    #[tokio::test]
    async fn adding_the_same_ssh_key_twice_does_not_duplicate_it() {
        let db = fresh_db().await;
        let u = db
            .create_user("sshdup", "sshdup@example.com", "h")
            .await
            .unwrap();
        let pk = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAmSSX13Ub9vIWCGFHUJAd04tQPCJxOTY7Gl1OerO+RE";

        let first = db.add_ssh_key(u.id, "laptop", pk).await.unwrap();
        let second = db.add_ssh_key(u.id, "desktop", pk).await.unwrap();

        assert_eq!(first.id, second.id, "the same key must map to one row");
        let keys = db.list_ssh_keys(u.id).await.unwrap();
        assert_eq!(keys.len(), 1, "expected one key, got {keys:?}");
        // The title is updated in place.
        assert_eq!(keys[0].title, "desktop");
        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn ssh_key_find_by_fingerprint() {
        let db = fresh_db().await;
        let u = db
            .create_user("sshfp", "sshfp@example.com", "h")
            .await
            .unwrap();
        let pk = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAmSSX13Ub9vIWCGFHUJAd04tQPCJxOTY7Gl1OerO+RE";
        let fingerprint = "SHA256:kbyIniKFUf4To+hc/phSRCjc5jOKJdNhVqdCnUblVjA";

        db.add_ssh_key(u.id, "ci-runner", pk).await.unwrap();

        // Matching fingerprint resolves to the key.
        let found = db.find_ssh_key_by_fingerprint(fingerprint).await.unwrap();
        assert!(found.is_some());
        let key = found.unwrap();
        assert_eq!(key.user_id, u.id);
        assert_eq!(key.title, "ci-runner");

        // Unknown fingerprint returns None.
        let missing = db
            .find_ssh_key_by_fingerprint("SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
            .await
            .unwrap();
        assert!(missing.is_none());
        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn ssh_key_validity() {
        // A real ed25519 OpenSSH public key is accepted.
        let valid =
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAmSSX13Ub9vIWCGFHUJAd04tQPCJxOTY7Gl1OerO+RE";
        assert!(ssh_key_is_valid(valid));

        // Trailing comment is allowed (fingerprint uses first two fields).
        let with_comment = format!("{valid} zac@example.com");
        assert!(ssh_key_is_valid(&with_comment));

        // Junk and malformed keys are rejected.
        assert!(!ssh_key_is_valid(""));
        assert!(!ssh_key_is_valid("ssh-ed25519"));
        assert!(!ssh_key_is_valid("ssh-ed25519 not-base64!!!"));
        assert!(!ssh_key_is_valid("just-a-word"));
    }

    #[tokio::test]
    async fn group_crud() {
        let db = fresh_db().await;
        let owner = db
            .create_user("groupowner", "groupowner@example.com", "h")
            .await
            .unwrap();
        let g = db
            .create_group("acme", "The Acme group", owner.id, None)
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
            .create_repository(
                owner.id,
                "acme",
                "widgets",
                "widget repo",
                false,
                "main",
                None,
            )
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
            .create_repository(
                owner.id,
                "issueowner",
                "issues-repo",
                "desc",
                false,
                "main",
                None,
            )
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
        sqlx::migrate!("./migrations").run(db.pool()).await.unwrap();
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
            .create_repository(owner.id, "commenter", "c-repo", "desc", false, "main", None)
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

        let _c2 = db
            .create_issue_comment(repo.id, issue.id, owner.id, &owner.username, "second")
            .await
            .unwrap();
        let list = db.list_issue_comments(issue.id, 10).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].body, "first!");
        assert_eq!(list[1].body, "second");

        // PR comment lifecycle
        let created_pr = db
            .create_pull_request(
                repo.id,
                "PR",
                "",
                "main",
                "main",
                owner.id,
                &owner.username,
                "a",
                "b",
            )
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
            "ssh_keys",
            "repo_pages",
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
        db.create_repository(alice.id, "alice2", "alpha", "", false, "main", None)
            .await
            .unwrap();
        db.create_repository(bob.id, "bob2", "beta", "", false, "main", None)
            .await
            .unwrap();
        db.create_repository(alice.id, "alice2", "gamma", "", false, "main", None)
            .await
            .unwrap();

        let alice_repos = db.list_repositories(Some("alice2"), 50).await.unwrap();
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
            .create_repository(user.id, "cici", "cirepo", "d", false, "main", None)
            .await
            .unwrap();

        // upsert_pipeline creates or refreshes.
        let pid = db
            .upsert_pipeline(
                repo.id,
                ".gita-ci.yml",
                "refs/heads/main",
                "abc123",
                "stages: [b]",
            )
            .await
            .unwrap();
        assert!(pid > 0);
        let fetched = db
            .get_pipeline_by_path(repo.id, ".gita-ci.yml")
            .await
            .unwrap();
        assert!(fetched.is_some());
        let p = fetched.unwrap();
        assert_eq!(p.ref_sha, "abc123");

        // Calling upsert again with the same path updates ref_sha + content
        // and returns the same id.
        let pid2 = db
            .upsert_pipeline(
                repo.id,
                ".gita-ci.yml",
                "refs/heads/main",
                "def456",
                "stages: [b, t]",
            )
            .await
            .unwrap();
        assert_eq!(pid, pid2);
        let p2 = db.get_pipeline(pid).await.unwrap().unwrap();
        assert_eq!(p2.ref_sha, "def456");

        // Insert a run, claim it, and assert state transitions.
        let run_id = db
            .insert_pipeline_run(
                pid,
                repo.id,
                "refs/heads/main",
                "def456",
                Some(user.id),
                None,
            )
            .await
            .unwrap();
        assert!(run_id > 0);

        let claimed = db
            .claim_pipeline_run("runner-1", 300)
            .await
            .unwrap()
            .unwrap();
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

        db.update_pipeline_run_status(run_id, "success")
            .await
            .unwrap();
        let finished = db.get_pipeline_run(run_id).await.unwrap().unwrap();
        assert_eq!(finished.status, "success");
        assert!(finished.finished_at.is_some());
        assert!(finished.claimed_until.is_none());

        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn pages_crud_roundtrip() {
        let db = fresh_db().await;
        let user = db
            .create_user("pagesowner", "pages@example.com", "h")
            .await
            .unwrap();
        let repo = db
            .create_repository(user.id, "pagesowner", "site", "desc", false, "main", None)
            .await
            .unwrap();

        // Absent by default.
        assert!(db.get_pages(repo.id).await.unwrap().is_none());

        // Insert.
        let created = db
            .upsert_pages(repo.id, true, "main", "/docs")
            .await
            .unwrap();
        assert!(created.enabled);
        assert_eq!(created.branch, "main");
        assert_eq!(created.path, "/docs");
        assert!(!created.created_at.is_empty());
        assert!(!created.updated_at.is_empty());

        let fetched = db
            .get_pages(repo.id)
            .await
            .unwrap()
            .expect("row should exist");
        assert_eq!(fetched.repo_id, repo.id);
        assert!(fetched.enabled);
        assert_eq!(fetched.branch, "main");
        assert_eq!(fetched.path, "/docs");

        // Upsert updates the same row rather than duplicating.
        let updated = db
            .upsert_pages(repo.id, false, "gh-pages", "/")
            .await
            .unwrap();
        assert!(!updated.enabled);
        assert_eq!(updated.branch, "gh-pages");
        assert_eq!(updated.path, "/");
        let all = db.get_pages(repo.id).await.unwrap().unwrap();
        assert_eq!(all.branch, "gh-pages");
        assert_eq!(all.path, "/");

        // Delete removes the row; a second delete reports no-op.
        assert!(db.delete_pages(repo.id).await.unwrap());
        assert!(db.get_pages(repo.id).await.unwrap().is_none());
        assert!(!db.delete_pages(repo.id).await.unwrap());

        drop_test_db(&db).await;
    }

    #[tokio::test]
    async fn pages_are_deleted_with_their_repository() {
        let db = fresh_db().await;
        let user = db
            .create_user("pagescascade", "pc@example.com", "h")
            .await
            .unwrap();
        let repo = db
            .create_repository(user.id, "pagescascade", "gone", "desc", false, "main", None)
            .await
            .unwrap();
        db.upsert_pages(repo.id, true, "main", "/").await.unwrap();

        db.delete_repository(repo.id).await.unwrap();

        // The FK's ON DELETE CASCADE must have removed the pages row.
        assert!(db.get_pages(repo.id).await.unwrap().is_none());
        let rows: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM repo_pages WHERE repo_id = $1")
            .bind(repo.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(rows.0, 0);

        drop_test_db(&db).await;
    }

    /// A reset token can be claimed exactly once: the second claim of the
    /// same token must find nothing.
    #[tokio::test]
    async fn password_reset_token_can_only_be_claimed_once() {
        let db = fresh_db().await;
        let user = db
            .create_user("resetonce", "ro@example.com", "h")
            .await
            .unwrap();
        let now = Utc::now();
        let expires = (now + chrono::Duration::hours(1)).to_rfc3339();
        db.create_password_reset_token(user.id, "tok-once", &expires)
            .await
            .unwrap();

        let first = db
            .claim_password_reset_token("tok-once", &now.to_rfc3339())
            .await
            .unwrap();
        assert_eq!(first, Some(user.id));

        // The token is spent now.
        let second = db
            .claim_password_reset_token("tok-once", &now.to_rfc3339())
            .await
            .unwrap();
        assert_eq!(second, None, "a reset token must be single-use");
        assert!(
            db.get_valid_password_reset_token("tok-once")
                .await
                .unwrap()
                .is_none(),
            "a claimed token must no longer read as valid"
        );

        drop_test_db(&db).await;
    }

    /// Concurrent claims of the same token must not both succeed.
    ///
    /// This is the case the old read-then-mark sequence got wrong: every
    /// caller passed the "is it still valid?" read and then went on to set
    /// a new password, the last one silently overwriting the others. The
    /// window between the read and the mark is wide — the Argon2 hash of
    /// the new password sits in it and takes ~575ms with Argon2's default
    /// parameters — so every concurrent request gets in.
    ///
    /// The atomic claim closes that window: the read and the mark are one
    /// statement, so exactly one caller can match the row.
    #[tokio::test]
    async fn concurrent_claims_of_one_token_yield_a_single_winner() {
        const CALLERS: usize = 8;

        let db = fresh_db().await;
        let user = db
            .create_user("resetrace", "rr@example.com", "h")
            .await
            .unwrap();
        let now = Utc::now();
        let expires = (now + chrono::Duration::hours(1)).to_rfc3339();
        db.create_password_reset_token(user.id, "tok-race", &expires)
            .await
            .unwrap();

        let now_str = now.to_rfc3339();
        let mut handles = Vec::new();
        for _ in 0..CALLERS {
            let db = db.clone();
            let now_str = now_str.clone();
            handles.push(tokio::spawn(async move {
                db.claim_password_reset_token("tok-race", &now_str).await
            }));
        }

        let mut winners = 0;
        for h in handles {
            if h.await.unwrap().unwrap().is_some() {
                winners += 1;
            }
        }
        assert_eq!(
            winners, 1,
            "exactly one claim may win, but {winners} of {CALLERS} did"
        );

        drop_test_db(&db).await;
    }

    /// An expired token cannot be claimed, and the expiry is compared
    /// chronologically — `expires_at` is a TEXT column, so a plain text
    /// comparison would order two timestamps of the same second wrongly.
    #[tokio::test]
    async fn expired_password_reset_token_cannot_be_claimed() {
        let db = fresh_db().await;
        let user = db
            .create_user("resetexp", "re@example.com", "h")
            .await
            .unwrap();
        let now = Utc::now();
        // Already in the past.
        let expires = (now - chrono::Duration::hours(1)).to_rfc3339();
        db.create_password_reset_token(user.id, "tok-expired", &expires)
            .await
            .unwrap();

        let claim = db
            .claim_password_reset_token("tok-expired", &now.to_rfc3339())
            .await
            .unwrap();
        assert_eq!(claim, None, "an expired token must not be claimable");

        drop_test_db(&db).await;
    }

    /// The purge keeps tokens that are still valid and removes the spent
    /// and expired ones.
    #[tokio::test]
    async fn purge_removes_spent_and_expired_tokens_but_keeps_live_ones() {
        let db = fresh_db().await;
        let user = db
            .create_user("resetpurge", "rp@example.com", "h")
            .await
            .unwrap();
        let now = Utc::now();
        let now_str = now.to_rfc3339();

        // Live: expires in an hour.
        db.create_password_reset_token(
            user.id,
            "tok-live",
            &(now + chrono::Duration::hours(1)).to_rfc3339(),
        )
        .await
        .unwrap();
        // Spent.
        db.create_password_reset_token(
            user.id,
            "tok-spent",
            &(now + chrono::Duration::hours(1)).to_rfc3339(),
        )
        .await
        .unwrap();
        db.mark_password_reset_token_used("tok-spent")
            .await
            .unwrap();
        // Expired.
        db.create_password_reset_token(
            user.id,
            "tok-old",
            &(now - chrono::Duration::hours(1)).to_rfc3339(),
        )
        .await
        .unwrap();

        db.purge_stale_password_reset_tokens(user.id, &now_str)
            .await
            .unwrap();

        let remaining: Vec<(String,)> =
            sqlx::query_as("SELECT token FROM password_reset_tokens WHERE user_id = $1")
                .bind(user.id)
                .fetch_all(db.pool())
                .await
                .unwrap();
        assert_eq!(
            remaining,
            vec![("tok-live".to_string(),)],
            "only the still-valid token may survive the purge"
        );

        drop_test_db(&db).await;
    }
}