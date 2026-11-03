//! Upgrade-path tests for the SQLx migration set.
//!
//! Every other test starts from an empty schema and applies the full
//! migration chain at once. That proves a fresh install works, but not
//! that an existing deployment can be upgraded: a migration that forgets
//! a default, rewrites a column, or fails on a populated table would only
//! surface on a real user's database.
//!
//! These tests apply a prefix of the chain (the schema an older release
//! shipped), insert rows through that older shape, then run the full
//! migrator exactly as the new server does and assert the data survived.

use std::borrow::Cow;
use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnection, PgPool, PgPoolOptions};
use sqlx::{Connection, Executor};

fn test_db_url() -> String {
    std::env::var("GITA_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://gita:gita@127.0.0.1:55432/gita".to_string())
}

fn all_migrations() -> Migrator {
    let mut migrator = sqlx::migrate!("./migrations");
    // Per-schema isolation means the database-scoped advisory lock would
    // only serialise tests that share nothing.
    migrator.locking = false;
    migrator
}

fn migrations_through(version: i64) -> Migrator {
    let all = all_migrations();
    let mut migrator = Migrator::DEFAULT;
    migrator.migrations = Cow::Owned(
        all.iter()
            .filter(|migration| migration.version <= version)
            .cloned()
            .collect(),
    );
    // Each test owns its own schema, so the database-scoped advisory lock
    // would only serialise unrelated tests.
    migrator.locking = false;
    migrator
}

/// Owns a schema-scoped pool and drops both the pool and the schema when
/// it goes out of scope, including on panic.
///
/// Closing the pool first matters: `DROP SCHEMA` blocks while another
/// session still has the schema on its `search_path`, which would turn
/// every cleanup into a ten-second timeout.
struct TestSchema {
    url: String,
    schema: String,
    pool: Option<PgPool>,
}

impl TestSchema {
    async fn new(schema: &str) -> Self {
        let url = test_db_url();
        let mut admin = PgConnection::connect(&url).await.expect("connect admin");
        admin
            .execute(format!("CREATE SCHEMA \"{schema}\"").as_str())
            .await
            .expect("create schema");
        let _ = admin.close().await;

        let search_path = schema.to_string();
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(move |conn, _meta| {
                let search_path = search_path.clone();
                Box::pin(async move {
                    conn.execute(format!("SET search_path TO \"{search_path}\"").as_str())
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .expect("connect schema pool");

        Self {
            url,
            schema: schema.to_string(),
            pool: Some(pool),
        }
    }

    fn pool(&self) -> &PgPool {
        self.pool.as_ref().expect("pool is alive")
    }
}

impl Drop for TestSchema {
    fn drop(&mut self) {
        let pool = self.pool.take();
        let url = self.url.clone();
        let schema = self.schema.clone();
        let worker = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async move {
                if let Some(pool) = pool {
                    let _ = tokio::time::timeout(Duration::from_secs(10), pool.close()).await;
                }
                if let Ok(mut conn) = PgConnection::connect(&url).await {
                    let _ = conn
                        .execute(format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE").as_str())
                        .await;
                    let _ = conn.close().await;
                }
            });
        });
        let _ = worker.join();
    }
}

fn unique_schema(prefix: &str) -> String {
    format!(
        "{prefix}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

async fn seed_old_release(pool: &PgPool) {
    sqlx::query(
        "INSERT INTO users (username, email, password_hash, created_at)
         VALUES ('legacy', 'legacy@example.com', 'hash', '2026-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .expect("seed user");

    sqlx::query(
        "INSERT INTO repositories (owner_id, owner_name, name, created_at)
         VALUES (1, 'legacy', 'old-repo', '2026-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .expect("seed repository");

    sqlx::query(
        "INSERT INTO issues (repo_id, number, title, author_id, author_name, created_at, updated_at)
         VALUES (1, 1, 'old issue', 1, 'legacy', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    )
    .execute(pool)
    .await
    .expect("seed issue");
}

/// Columns that have existed since the first release. Used to prove the
/// seed really landed before the upgrade runs.
async fn assert_seed_present(pool: &PgPool) {
    let user: (String,) = sqlx::query_as("SELECT username FROM users WHERE username = 'legacy'")
        .fetch_one(pool)
        .await
        .expect("the legacy user must exist before the upgrade");
    assert_eq!(user.0, "legacy");

    let repo: (String,) = sqlx::query_as("SELECT name FROM repositories WHERE name = 'old-repo'")
        .fetch_one(pool)
        .await
        .expect("the legacy repository must exist before the upgrade");
    assert_eq!(repo.0, "old-repo");
}

async fn assert_seed_survived(pool: &PgPool) {
    let user: (String, String, bool, String) = sqlx::query_as(
        "SELECT username, email, is_admin, locale FROM users WHERE username = 'legacy'",
    )
    .fetch_one(pool)
    .await
    .expect("the legacy user must survive the upgrade");
    assert_eq!(user.0, "legacy");
    assert_eq!(user.1, "legacy@example.com");
    assert!(!user.2, "the admin flag must default to false");
    assert_eq!(user.3, "zh", "the locale must default to zh");

    let repo: (String, String) =
        sqlx::query_as("SELECT owner_name, name FROM repositories WHERE name = 'old-repo'")
            .fetch_one(pool)
            .await
            .expect("the legacy repository must survive the upgrade");
    assert_eq!(repo, ("legacy".to_string(), "old-repo".to_string()));

    let issue: (String,) = sqlx::query_as("SELECT title FROM issues WHERE number = 1")
        .fetch_one(pool)
        .await
        .expect("the legacy issue must survive the upgrade");
    assert_eq!(issue.0, "old issue");
}

async fn applied_versions(pool: &PgPool) -> Vec<i64> {
    sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(pool)
        .await
        .expect("read applied migrations")
}

/// The chain is strictly ordered: SQLx refuses duplicate versions, but it
/// would happily accept a migration that sorts before its predecessor.
#[test]
fn migration_versions_are_strictly_increasing() {
    let all = all_migrations();
    let mut previous = None;
    for migration in all.iter() {
        if let Some(previous) = previous {
            assert!(
                migration.version > previous,
                "migration {} sorts before its predecessor {previous}",
                migration.version
            );
        }
        previous = Some(migration.version);
    }
    assert!(all.iter().next().is_some(), "there must be migrations");
}

/// A fresh schema can run the whole chain, and re-running it is a no-op.
#[tokio::test]
async fn the_full_chain_is_idempotent() {
    let schema = unique_schema("gita_mig_idem");
    let db = TestSchema::new(&schema).await;
    let pool = db.pool();

    let all = all_migrations();
    all.run(pool).await.expect("initial migration run");
    let first = applied_versions(pool).await;
    assert_eq!(
        first.len(),
        all.iter().count(),
        "every migration should be recorded"
    );

    all.run(pool).await.expect("second migration run");
    let second = applied_versions(pool).await;
    assert_eq!(first, second, "a second run must not apply anything");

    let enrollment: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('vm_enrollment_tokens')::text")
            .fetch_one(pool)
            .await
            .expect("look up the newest table");
    assert!(
        enrollment.is_some_and(|name| name.contains("vm_enrollment_tokens")),
        "the newest migration must have created its table"
    );
}

/// Every migration applies on top of the previous one, and rows written
/// under the oldest schema survive each step.
///
/// Running the chain one migration at a time (rather than re-running a
/// growing prefix on a fresh schema per sample) keeps the test fast while
/// covering every migration, not just a few snapshots.
#[tokio::test]
async fn every_migration_applies_on_top_of_the_previous_one_and_keeps_rows() {
    let all = all_migrations();
    let db = TestSchema::new(&unique_schema("gita_mig_walk")).await;
    let pool = db.pool();

    for (index, migration) in all.iter().enumerate() {
        let mut one = Migrator::DEFAULT;
        one.migrations = Cow::Owned(vec![migration.clone()]);
        one.locking = false;
        // Earlier versions are already applied; only this one is pending.
        one.ignore_missing = true;
        one.run(pool)
            .await
            .unwrap_or_else(|error| panic!("migration {} failed: {error}", migration.version));

        if index == 0 {
            seed_old_release(pool).await;
        }
        // The seed columns exist from the first migration onwards.
        assert_seed_present(pool).await;
        assert_eq!(
            applied_versions(pool).await.len(),
            index + 1,
            "exactly one migration should be applied per step"
        );
    }

    // The columns added by later migrations default the old rows correctly.
    assert_seed_survived(pool).await;
    assert_eq!(
        applied_versions(pool).await.len(),
        all.iter().count(),
        "every migration must be applied"
    );

    // A normal startup after the incremental upgrade must be a no-op.
    all.run(pool).await.expect("restart after the walk");
    assert_seed_survived(pool).await;
}

/// The newest migration must be reachable from the release immediately
/// before it, which is the only upgrade path real users take today.
#[tokio::test]
async fn the_latest_upgrade_path_works_from_the_previous_release() {
    let all = all_migrations();
    let previous = all
        .iter()
        .rev()
        .nth(1)
        .expect("at least two migrations")
        .version;

    let schema = unique_schema("gita_mig_latest");
    let db = TestSchema::new(&schema).await;
    let pool = db.pool();

    migrations_through(previous)
        .run(pool)
        .await
        .expect("migrate to the previous release");
    seed_old_release(pool).await;
    assert_seed_present(pool).await;

    all.run(pool).await.expect("upgrade to the latest release");
    assert_seed_survived(pool).await;

    let tables: Vec<(String,)> = sqlx::query_as(
        "SELECT table_name FROM information_schema.tables
         WHERE table_schema = current_schema()
           AND table_name IN ('vm_node_credentials', 'vm_enrollment_tokens')
         ORDER BY table_name",
    )
    .fetch_all(pool)
    .await
    .expect("list the newest tables");
    assert_eq!(
        tables,
        vec![
            ("vm_enrollment_tokens".to_string(),),
            ("vm_node_credentials".to_string(),),
        ]
    );
}
