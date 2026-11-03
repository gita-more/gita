//! Race tests for the operations that several workers perform at once.
//!
//! A single-threaded test proves a query returns the right row; it cannot
//! prove two runners never receive the same row. `claim_pipeline_run` is
//! the hand-off point between the queue and every runner process, so a
//! double claim means a job runs twice and two workers write to the same
//! log stream.

use gita_db::{drop_test_db, new_test_db, Db};

fn test_db_url() -> String {
    std::env::var("GITA_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://gita:gita@127.0.0.1:55432/gita".to_string())
}

async fn db_with_repo() -> (Db, i64) {
    let db = new_test_db(&test_db_url()).await;
    let owner = db
        .create_user("runner-owner", "runner-owner@example.com", "hash")
        .await
        .expect("create user");
    let repo = db
        .create_repository(owner.id, "runner-owner", "raced", "", false, "main", None)
        .await
        .expect("create repository");
    (db, repo.id)
}

async fn queue_runs(db: &Db, repo_id: i64, count: usize) -> Vec<i64> {
    let pipeline = db
        .upsert_pipeline(repo_id, ".gita-ci.yml", "main", "sha", "stages: []")
        .await
        .expect("create pipeline");
    let mut ids = Vec::with_capacity(count);
    for i in 0..count {
        let id = db
            .insert_pipeline_run(pipeline, repo_id, "main", &format!("sha-{i}"), None, None)
            .await
            .expect("enqueue run");
        ids.push(id);
    }
    ids
}

/// Many runners claim at once. Each queued run must go to exactly one of
/// them, with no duplicates and none left behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_runners_never_claim_the_same_run_twice() {
    const RUNS: usize = 64;
    const RUNNERS: usize = 8;

    let (db, repo_id) = db_with_repo().await;
    let queued = queue_runs(&db, repo_id, RUNS).await;

    let mut tasks = Vec::with_capacity(RUNNERS);
    for runner in 0..RUNNERS {
        let db = db.clone();
        tasks.push(tokio::spawn(async move {
            let name = format!("runner-{runner}");
            let mut claimed = Vec::new();
            // Drain until the queue is empty. The loop terminates because
            // every successful claim removes one row from the queue.
            while let Some(run) = db
                .claim_pipeline_run(&name, 300)
                .await
                .expect("claim must not error")
            {
                claimed.push(run.id);
            }
            claimed
        }));
    }

    let mut all_claimed = Vec::new();
    for task in tasks {
        all_claimed.extend(task.await.expect("claimer task"));
    }

    all_claimed.sort_unstable();
    let mut expected = queued.clone();
    expected.sort_unstable();
    assert_eq!(
        all_claimed, expected,
        "every run must be claimed exactly once, by some runner"
    );

    // Every claimed row is now running under a named owner.
    for run_id in queued {
        let run = db
            .get_pipeline_run(run_id)
            .await
            .expect("read run")
            .expect("run exists");
        assert_eq!(run.status, "running");
        assert!(
            run.claimed_by_runner_id.is_some(),
            "run {run_id} must record its runner"
        );
    }

    drop_test_db(&db).await;
}

/// A run that is already running is never handed to a second runner, even
/// after its lease timestamp has passed.
///
/// Reclaiming a live run would execute its jobs twice, which is worse than
/// leaving a crashed runner's run for an explicit operator requeue. The
/// lease is a visibility deadline, not permission to duplicate work.
#[tokio::test]
async fn a_running_run_is_not_stolen_when_its_lease_expires() {
    let (db, repo_id) = db_with_repo().await;
    let queued = queue_runs(&db, repo_id, 1).await;
    let run_id = queued[0];

    let first = db
        .claim_pipeline_run("dead-runner", -1)
        .await
        .expect("claim")
        .expect("a run must be available");
    assert_eq!(first.id, run_id);
    assert_eq!(
        first.claimed_by_runner_id.as_deref(),
        Some("dead-runner"),
        "the first claim owns the run"
    );

    let second = db
        .claim_pipeline_run("live-runner", 300)
        .await
        .expect("second claim");
    assert!(
        second.is_none(),
        "a running run must not be stolen from its owner"
    );

    drop_test_db(&db).await;
}
