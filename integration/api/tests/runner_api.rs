//! Tests for the runner API (`/api/runner/*`).
//!
//! The runner used to talk to Postgres directly; these endpoints are what
//! replaced those database calls, and what they must guarantee:
//!
//! * **kind check** — a user's everyday PAT cannot claim CI work, only a
//!   token named `runner/…` can;
//! * **claim atomicity** — one run goes to exactly one runner, no matter
//!   how many poll at once;
//! * **log idempotency** — a retried log batch does not duplicate lines.

mod common;

use axum::http::Method;

/// Enqueue a run for an existing `owner/repo` and return its id.
async fn enqueue_existing_repo(app: &gita_admin::App, owner: &str, name: &str, yaml: &str) -> i64 {
    let db = app.db.clone();
    common::push_initial_commit(app, owner, name);

    let repo = db
        .get_repository_by_name(owner, name)
        .await
        .unwrap()
        .expect("repo exists");
    let head_sha = app
        .git
        .head_sha(owner, name, "refs/heads/main")
        .expect("head sha");
    let pipeline_id = db
        .upsert_pipeline(repo.id, ".gita-ci.yml", "refs/heads/main", &head_sha, yaml)
        .await
        .unwrap();
    db.insert_pipeline_run(
        pipeline_id,
        repo.id,
        "refs/heads/main",
        &head_sha,
        None,
        None,
    )
    .await
    .unwrap()
}

/// Register `owner`, create `owner/name`, and enqueue a run for it.
async fn enqueue_run(app: &gita_admin::App, owner: &str, name: &str, yaml: &str) -> i64 {
    let user = common::register_user(app, owner).await;
    common::create_repo(app, &user.token, name).await;
    enqueue_existing_repo(app, owner, name, yaml).await
}

#[tokio::test]
async fn a_non_runner_token_cannot_claim_runs() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "runguard").await;
    common::create_repo(&app, &user.token, "guardedrepo").await;
    let run_id = enqueue_existing_repo(&app, "runguard", "guardedrepo", "stages: [build]\n").await;

    // An ordinary user PAT is not a runner token.
    let (status, token_body) = common::request(
        &app,
        Method::POST,
        "/api/auth/tokens",
        Some(serde_json::json!({ "name": "not-a-runner" })),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let user_pat = token_body["token"].as_str().expect("token");

    let (status, _body) = common::request(
        &app,
        Method::POST,
        "/api/runner/claim",
        Some(serde_json::json!({ "runner_name": "intruder" })),
        Some(user_pat),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::FORBIDDEN,
        "user tokens must not claim runs"
    );

    // No token at all.
    let (status, _body) =
        common::request(&app, Method::POST, "/api/runner/claim", None, None).await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);

    // The run is still there — nobody claimed it.
    let run = app.db.get_pipeline_run(run_id).await.unwrap().unwrap();
    assert_eq!(run.status, "queued");
}

#[tokio::test]
async fn exactly_one_runner_wins_a_claim_race() {
    let (app, _guard) = common::test_app().await;
    let _ = enqueue_run(&app, "raceci", "racerepo", "stages: [build]\n").await;
    let db = app.db.clone();
    let pipeline = db
        .get_pipeline_by_path(
            db.get_repository_by_name("raceci", "racerepo")
                .await
                .unwrap()
                .unwrap()
                .id,
            ".gita-ci.yml",
        )
        .await
        .unwrap()
        .unwrap();
    let run_id = db.list_pipeline_runs(pipeline.id, 10).await.unwrap()[0].id;

    let token = common::runner_token(&app, "raceci").await;

    // Both runners claim concurrently.
    let body = |name: &str| Some(serde_json::json!({ "runner_name": name }));
    let (s1, b1) = common::request(
        &app,
        Method::POST,
        "/api/runner/claim",
        body("runner-a"),
        Some(&token),
    )
    .await;
    let (s2, b2) = common::request(
        &app,
        Method::POST,
        "/api/runner/claim",
        body("runner-b"),
        Some(&token),
    )
    .await;
    assert_eq!(s1, axum::http::StatusCode::OK);
    assert_eq!(s2, axum::http::StatusCode::OK);

    let claimed: Vec<i64> = [&b1, &b2]
        .iter()
        .filter_map(|v| v["run"]["id"].as_i64())
        .collect();
    assert_eq!(
        claimed,
        vec![run_id],
        "exactly one runner must get the run, got {claimed:?}"
    );

    // The run is running for the winner and untouched by the loser.
    let run = db.get_pipeline_run(run_id).await.unwrap().unwrap();
    assert_eq!(run.status, "running");
    assert_eq!(run.claimed_by_runner_id.as_deref(), Some("runner-a"));
}

#[tokio::test]
async fn logs_are_idempotent_per_seq() {
    let (app, _guard) = common::test_app().await;
    let run_id = enqueue_run(&app, "logci", "logrepo", "stages: [build]\n").await;
    let db = app.db.clone();
    let token = common::runner_token(&app, "logci").await;

    // A job row to write logs to.
    let job_id = db
        .insert_job(run_id, "build", "build", &[], &[])
        .await
        .unwrap();

    let logs = |seqs: Vec<i64>| {
        Some(serde_json::json!(seqs
            .iter()
            .map(|seq| {
                serde_json::json!({
                    "seq": seq,
                    "stream": "stdout",
                    "line": format!("line-{seq}"),
                    "timestamp": "2026-01-01T00:00:00Z",
                })
            })
            .collect::<Vec<_>>()))
    };
    let uri = format!("/api/runner/jobs/{job_id}/logs");

    // A batch, then the very same batch again — a retry.
    let (status, _) =
        common::request(&app, Method::POST, &uri, logs(vec![0, 1, 2]), Some(&token)).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let (status, _) =
        common::request(&app, Method::POST, &uri, logs(vec![0, 1, 2]), Some(&token)).await;
    assert_eq!(status, axum::http::StatusCode::OK);

    let rows = db.list_logs_since(job_id, 0, 100).await.unwrap();
    assert_eq!(
        rows.len(),
        3,
        "a retried batch must not duplicate lines; got {rows:?}"
    );
}

#[tokio::test]
async fn an_invalid_stream_name_is_rejected() {
    let (app, _guard) = common::test_app().await;
    let _user = common::register_user(&app, "streamci").await;
    let token = common::runner_token(&app, "streamci").await;
    let (status, _) = common::request(
        &app,
        Method::POST,
        "/api/runner/jobs/1/logs",
        Some(serde_json::json!([{
            "seq": 0, "stream": "stdout; DROP TABLE jobs", "line": "x"
        }])),
        Some(&token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
}
