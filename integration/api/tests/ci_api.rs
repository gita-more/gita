mod common;

use axum::http::Method;

use common::push_ci_yml;

#[tokio::test]
async fn manual_trigger_creates_new_run() {
    let (app, _guard) = common::test_app().await;
    let db = app.db.clone();

    let owner_user = common::register_user(&app, "triggerowner").await;
    let other_user = common::register_user(&app, "triggerother").await;
    let _full = common::create_repo(&app, &owner_user.token, "triggerrepo").await;
    common::push_initial_commit(&app, "triggerowner", "triggerrepo");
    push_ci_yml(
        &app,
        "triggerowner",
        "triggerrepo",
        r#"
stages: [build]
jobs:
  hello:
    stage: build
    script: [echo "triggered"]
"#,
    );

    // Pre-create a pipeline row so the trigger has something to enqueue
    // against. We have to call the helper directly because the trigger
    // endpoint only enqueues; it does not upsert pipelines.
    let (status, repo_json) = common::request(
        &app,
        Method::GET,
        "/api/repos/triggerowner/triggerrepo",
        None,
        Some(&owner_user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let repo_id = repo_json["id"].as_i64().unwrap();
    let head_sha = app
        .git
        .head_sha("triggerowner", "triggerrepo", "refs/heads/main")
        .expect("head_sha");
    db.upsert_pipeline(
        repo_id,
        ".gita-ci.yml",
        "refs/heads/main",
        &head_sha,
        "stages: [build]",
    )
    .await
    .unwrap();

    // POST /api/repos/.../pipelines/.gita-ci.yml/runs as the owner.
    let (status, body) = common::request(
        &app,
        Method::POST,
        "/api/repos/triggerowner/triggerrepo/pipelines/.gita-ci.yml/runs",
        None,
        Some(&owner_user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "trigger: {body}");
    let triggered_run_id = body["run"]["id"].as_i64().unwrap();
    assert_eq!(body["run"]["status"], "queued");
    assert_eq!(body["run"]["head_sha"], head_sha);

    // The new run is observable via list_runs.
    let (status, list_body) = common::request(
        &app,
        Method::GET,
        "/api/repos/triggerowner/triggerrepo/pipelines/.gita-ci.yml/runs",
        None,
        Some(&owner_user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let listed: Vec<i64> = list_body["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    assert!(
        listed.contains(&triggered_run_id),
        "newly-triggered run {triggered_run_id} should appear in list; got {listed:?}"
    );

    // A non-owner is refused. The answer is 404, not 403: a refusal must
    // not tell the caller that the repository exists.
    let (status, _) = common::request(
        &app,
        Method::POST,
        "/api/repos/triggerowner/triggerrepo/pipelines/.gita-ci.yml/runs",
        None,
        Some(&other_user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    // Missing pipeline -> 404.
    let (status, _) = common::request(
        &app,
        Method::POST,
        "/api/repos/triggerowner/triggerrepo/pipelines/nonexistent.yml/runs",
        None,
        Some(&owner_user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    // Unauthenticated -> 401 (handler requires AuthUserExtractor).
    let (status, _) = common::request(
        &app,
        Method::POST,
        "/api/repos/triggerowner/triggerrepo/pipelines/.gita-ci.yml/runs",
        None,
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
}

/// Events published through the ordinary `bus.publish` path must still be
/// stored — the "already persisted" marker must not swallow them.
#[tokio::test]
async fn ordinary_published_events_are_still_persisted() {
    let (app, guard) = common::test_app().await;
    let schema = guard.schema.clone();
    let user = common::register_user(&app, "pubevent").await;
    common::create_repo(&app, &user.token, "cirepo").await;

    // Creating a repository publishes a RepoCreated event through the
    // normal path.
    let repo_id = app
        .db
        .get_repository_by_name("pubevent", "cirepo")
        .await
        .unwrap()
        .expect("repo")
        .id;

    let pool = sqlx::PgPool::connect(&common::test_db_url()).await.unwrap();
    let mut count: (i64,) = (0,);
    for _ in 0..50 {
        count = sqlx::query_as(&format!(
            "SELECT count(*) FROM \"{schema}\".events
             WHERE event_type = 'repo_created' AND repo_id = $1"
        ))
        .bind(repo_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        if count.0 > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        count.0, 1,
        "an ordinarily published event must be stored exactly once, found {} row(s)",
        count.0
    );
}

/// `GET /api/repos/{owner}/{repo}/pipelines` must list the pipeline defined
/// by the repo's `.gita-ci.yml`.
///
/// Regression guard: the handler called
/// `read_file(owner, repo, ".gita-ci.yml", &head)` — reference and path
/// swapped — so the lookup never resolved and this endpoint always returned
/// an empty list. The repo's CI configuration was real and the push trigger
/// really did enqueue a run; only the listing was broken, and nothing
/// exercised it.
#[tokio::test]
async fn pipelines_endpoint_lists_the_repo_ci_config() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "listpip").await;
    common::create_repo(&app, &user.token, "cirepo").await;
    common::push_files(
        &app,
        "listpip",
        "cirepo",
        &[(
            ".gita-ci.yml",
            b"stages: [build]\njobs:\n  build:\n    stage: build\n    script:\n      - echo hi\n"
                .as_slice(),
        )],
        "main",
        "add ci config",
    );

    let (status, body) = common::request(
        &app,
        axum::http::Method::GET,
        "/api/repos/listpip/cirepo/pipelines",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "pipelines: {body}");
    let pipelines = body["pipelines"].as_array().expect("pipelines array");
    assert_eq!(
        pipelines.len(),
        1,
        "the repo's .gita-ci.yml should be listed, got {body}"
    );
    assert_eq!(pipelines[0]["path"], ".gita-ci.yml");
    assert!(
        !pipelines[0]["ref_sha"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "the listed pipeline should record the ref it was read at"
    );
}
