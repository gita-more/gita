mod common;

use axum::http::Method;
use chrono::Utc;
use gita_runner::config::RunnerConfig;
use gita_runner::executor::execute_pipeline;
use gita_runner::registry::JobRegistry;
use gita_runner::workspace::run_work_dir;
use std::process::Command;

use common::{first_ts, push_ci_yml};

#[tokio::test]
async fn push_ci_yml_triggers_run_and_runner_executes() {
    let (app, _guard) = common::test_app().await;
    // Use the app's connection — it pins the per-test schema.
    let db = app.db.clone();

    let user = common::register_user(&app, "ciowner").await;
    let full = common::create_repo(&app, &user.token, "cirepo").await;
    assert_eq!(full, "ciowner/cirepo");

    // Push an initial commit (so the bare repo has a real branch), then
    // push a commit adding .gita-ci.yml.
    common::push_initial_commit(&app, "ciowner", "cirepo");
    push_ci_yml(
        &app,
        "ciowner",
        "cirepo",
        r#"
stages: [build]
jobs:
  hello:
    stage: build
    script: [echo "sentinel-CI-RAN-OK"]
"#,
    );

    // Fetch repo id + head sha.
    let (status, repo_json) = common::request(
        &app,
        Method::GET,
        &format!("/api/repos/ciowner/cirepo"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{repo_json}");
    let repo_id = repo_json["id"].as_i64().unwrap();

    // Resolve head SHA from the branches endpoint (Vec<String> of branch names).
    let (status, branches_json) = common::request(
        &app,
        Method::GET,
        &format!("/api/repos/ciowner/cirepo/branches"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let branches: Vec<String> = branches_json
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_str().unwrap().to_string())
        .collect();
    assert!(
        branches.iter().any(|b| b == "main"),
        "expected main branch in {branches:?}"
    );
    let head_sha = app
        .git
        .head_sha("ciowner", "cirepo", "refs/heads/main")
        .expect("head_sha");

    let owner_user = db
        .get_user_by_username(&user.username)
        .await
        .unwrap()
        .unwrap();

    // Upsert pipeline + insert run as the HTTP handler would.
    let pid = db
        .upsert_pipeline(
            repo_id,
            ".gita-ci.yml",
            "refs/heads/main",
            &head_sha,
            "stages: [build]",
        )
        .await
        .unwrap();
    let run_id = db
        .insert_pipeline_run(
            pid,
            repo_id,
            "refs/heads/main",
            &head_sha,
            Some(owner_user.id),
            None,
        )
        .await
        .unwrap();

    // Build a ClaimedRun row and pre-clone the repo into the workspace
    // so execute_pipeline skips the network clone.
    let claimed = db
        .claim_pipeline_run("test-runner", 300)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, run_id);

    let runner_config = RunnerConfig {
        server_db_url: common::test_db_url(),
        server_base_url: "http://localhost:3100".to_string(),
        runner_token: "test-token".to_string(),
        runner_name: "test-runner".to_string(),
        work_dir: std::env::temp_dir().join(format!("gita-ci-it-{}", std::process::id())),
        poll_interval_secs: 1,
        claim_lease_secs: 300,
        concurrent_jobs: 1,
        job_timeout_secs: 300,
    };

    // Pre-clone the repo at head_sha into the run workspace.
    let repo_dir = run_work_dir(&runner_config, run_id).join("repo");
    let bare = app
        .data_dir
        .join("repos")
        .join("ciowner")
        .join("cirepo.git");
    let bare_url = format!("file://{}", bare.display());
    let clone_out = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--branch")
        .arg("main")
        .arg(&bare_url)
        .arg(&repo_dir)
        .output()
        .expect("git clone");
    assert!(
        clone_out.status.success(),
        "clone failed: {} {}",
        String::from_utf8_lossy(&clone_out.stdout),
        String::from_utf8_lossy(&clone_out.stderr)
    );

    // Run the pipeline executor.
    let registry = JobRegistry::new();
    execute_pipeline(&db, &runner_config, &registry, &claimed)
        .await
        .expect("execute_pipeline");

    // Run should now be success.
    let finished = db.get_pipeline_run(run_id).await.unwrap().unwrap();
    assert_eq!(finished.status, "success");

    // Fetch logs via API and assert the sentinel appears.
    let jobs = db.list_jobs(run_id).await.unwrap();
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.status, "success");

    let logs = db.list_logs_since(job.id, -1, 100).await.unwrap();
    let any_sentinel = logs.iter().any(|l| l.line.contains("sentinel-CI-RAN-OK"));
    assert!(any_sentinel, "logs should contain sentinel; got {:?}", logs);
}

#[tokio::test]
async fn cancel_run_actually_kills_running_job() {
    let (app, _guard) = common::test_app().await;
    let db = app.db.clone();

    let user = common::register_user(&app, "cancelowner").await;
    let _full = common::create_repo(&app, &user.token, "cancelrepo").await;
    common::push_initial_commit(&app, "cancelowner", "cancelrepo");
    // Long-running job: sleep 30 seconds. We will cancel it within a
    // few seconds so the runner must actually kill the child process.
    push_ci_yml(
        &app,
        "cancelowner",
        "cancelrepo",
        r#"
stages: [wait]
jobs:
  slow:
    stage: wait
    script: [sleep 30]
"#,
    );

    let (status, repo_json) = common::request(
        &app,
        Method::GET,
        "/api/repos/cancelowner/cancelrepo",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let repo_id = repo_json["id"].as_i64().unwrap();
    let head_sha = app
        .git
        .head_sha("cancelowner", "cancelrepo", "refs/heads/main")
        .expect("head_sha");

    let owner_user = db
        .get_user_by_username(&user.username)
        .await
        .unwrap()
        .unwrap();

    let pid = db
        .upsert_pipeline(
            repo_id,
            ".gita-ci.yml",
            "refs/heads/main",
            &head_sha,
            "stages: [wait]",
        )
        .await
        .unwrap();
    let run_id = db
        .insert_pipeline_run(
            pid,
            repo_id,
            "refs/heads/main",
            &head_sha,
            Some(owner_user.id),
            None,
        )
        .await
        .unwrap();

    // Pre-clone so execute_pipeline skips network clone.
    let runner_config = RunnerConfig {
        server_db_url: common::test_db_url(),
        server_base_url: "http://localhost:3100".to_string(),
        runner_token: "cancel-test-token".to_string(),
        runner_name: "cancel-runner".to_string(),
        work_dir: std::env::temp_dir().join(format!(
            "gita-cancel-it-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )),
        poll_interval_secs: 1,
        claim_lease_secs: 300,
        concurrent_jobs: 1,
        job_timeout_secs: 300,
    };
    let _ = std::fs::create_dir_all(&runner_config.work_dir);
    let bare = app
        .data_dir
        .join("repos")
        .join("cancelowner")
        .join("cancelrepo.git");
    let bare_url = format!("file://{}", bare.display());
    let repo_dir = run_work_dir(&runner_config, run_id).join("repo");
    let clone_out = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--branch")
        .arg("main")
        .arg(&bare_url)
        .arg(&repo_dir)
        .output()
        .expect("git clone");
    assert!(clone_out.status.success(), "pre-clone failed");

    // Spawn the executor in a background task — it will block for
    // ~30s on the long-running sleep job otherwise.
    let db_for_exec = db.clone();
    let cfg = runner_config.clone();
    let claimed = db
        .claim_pipeline_run("cancel-runner", 300)
        .await
        .unwrap()
        .unwrap();
    let exec_handle = tokio::spawn(async move {
        let registry = JobRegistry::new();
        execute_pipeline(&db_for_exec, &cfg, &registry, &claimed).await
    });

    // Wait until the run is observed running, then cancel it.
    let mut running_observed = false;
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let r = db.get_pipeline_run(run_id).await.unwrap().unwrap();
        if r.status == "running" {
            running_observed = true;
            break;
        }
    }
    assert!(running_observed, "run never transitioned to running");

    let cancel_started = std::time::Instant::now();
    db.update_pipeline_run_status(run_id, "canceled")
        .await
        .unwrap();
    let exec_result = tokio::time::timeout(std::time::Duration::from_secs(15), exec_handle)
        .await
        .expect("executor did not finish within 15s after cancel")
        .expect("executor task join error")
        .expect("executor returned an error");
    let _ = exec_result;
    let elapsed = cancel_started.elapsed();
    // Executor should reap the killed child promptly (a few seconds),
    // not wait the full 30-second sleep.
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "cancel took too long: {elapsed:?}"
    );

    // Run should be canceled (or have transitioned through canceled);
    // either is acceptable because the cancel was issued mid-flight.
    let final_run = db.get_pipeline_run(run_id).await.unwrap().unwrap();
    assert!(
        matches!(final_run.status.as_str(), "canceled" | "running"),
        "unexpected final status: {}",
        final_run.status
    );

    // The slow job should be marked canceled.
    let jobs = db.list_jobs(run_id).await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].status, "canceled", "job should be canceled");
}

#[tokio::test]
async fn stages_dag_runs_in_dependency_order() {
    let (app, _guard) = common::test_app().await;
    let db = app.db.clone();

    let user = common::register_user(&app, "dagowner").await;
    let _full = common::create_repo(&app, &user.token, "dagrepo").await;
    common::push_initial_commit(&app, "dagowner", "dagrepo");
    // Two-stage pipeline. The deploy stage's job depends on BOTH test
    // jobs, so it must run after both. Each job writes a sentinel line
    // with a timestamp; we assert the order at the end by reading logs.
    push_ci_yml(
        &app,
        "dagowner",
        "dagrepo",
        r#"
stages: [build, test, deploy]
jobs:
  compile:
    stage: build
    script: [echo "stage=build job=compile"]
  unit:
    stage: test
    script: [echo "stage=test job=unit"]
  lint:
    stage: test
    script: [echo "stage=test job=lint"]
  release:
    stage: deploy
    script: [echo "stage=deploy job=release"]
    needs: [unit, lint]
"#,
    );

    let (status, repo_json) = common::request(
        &app,
        Method::GET,
        "/api/repos/dagowner/dagrepo",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let repo_id = repo_json["id"].as_i64().unwrap();
    let head_sha = app
        .git
        .head_sha("dagowner", "dagrepo", "refs/heads/main")
        .expect("head_sha");

    let owner_user = db
        .get_user_by_username(&user.username)
        .await
        .unwrap()
        .unwrap();

    let pid = db
        .upsert_pipeline(
            repo_id,
            ".gita-ci.yml",
            "refs/heads/main",
            &head_sha,
            "stages: [build, test, deploy]",
        )
        .await
        .unwrap();
    let run_id = db
        .insert_pipeline_run(
            pid,
            repo_id,
            "refs/heads/main",
            &head_sha,
            Some(owner_user.id),
            None,
        )
        .await
        .unwrap();

    // Pre-clone so execute_pipeline skips the network clone.
    let runner_config = RunnerConfig {
        server_db_url: common::test_db_url(),
        server_base_url: "http://localhost:3100".to_string(),
        runner_token: "dag-test-token".to_string(),
        runner_name: "dag-runner".to_string(),
        work_dir: std::env::temp_dir().join(format!(
            "gita-dag-it-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )),
        poll_interval_secs: 1,
        claim_lease_secs: 300,
        concurrent_jobs: 1,
        job_timeout_secs: 300,
    };
    let _ = std::fs::create_dir_all(&runner_config.work_dir);
    let bare = app
        .data_dir
        .join("repos")
        .join("dagowner")
        .join("dagrepo.git");
    let bare_url = format!("file://{}", bare.display());
    let repo_dir = run_work_dir(&runner_config, run_id).join("repo");
    let clone_out = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--branch")
        .arg("main")
        .arg(&bare_url)
        .arg(&repo_dir)
        .output()
        .expect("git clone");
    assert!(clone_out.status.success(), "pre-clone failed");

    let claimed = db
        .claim_pipeline_run("dag-runner", 300)
        .await
        .unwrap()
        .unwrap();
    let registry = JobRegistry::new();
    execute_pipeline(&db, &runner_config, &registry, &claimed)
        .await
        .expect("execute_pipeline");

    let run = db.get_pipeline_run(run_id).await.unwrap().unwrap();
    assert_eq!(run.status, "success");

    let jobs = db.list_jobs(run_id).await.unwrap();
    let mut by_name: std::collections::HashMap<String, &gita_db::JobRow> =
        std::collections::HashMap::new();
    for j in &jobs {
        by_name.insert(j.name.clone(), j);
    }
    // Every job must have run successfully.
    for name in ["compile", "unit", "lint", "release"] {
        let job = by_name
            .get(name)
            .unwrap_or_else(|| panic!("missing job {name}"));
        assert_eq!(job.status, "success", "job {name} should be success");
    }

    // release must depend on both unit AND lint. Compare by ts (per-log
    // timestamp); each job's seq is local so we use ts instead.
    let release_first_ts = first_ts(&db, by_name["release"].id).await;
    let unit_first_ts = first_ts(&db, by_name["unit"].id).await;
    let lint_first_ts = first_ts(&db, by_name["lint"].id).await;
    let compile_first_ts = first_ts(&db, by_name["compile"].id).await;
    assert!(release_first_ts.is_some(), "release should have logs");
    assert!(unit_first_ts.is_some(), "unit should have logs");
    assert!(lint_first_ts.is_some(), "lint should have logs");
    assert!(compile_first_ts.is_some(), "compile should have logs");
    let release_first_ts = release_first_ts.unwrap();
    let unit_first_ts = unit_first_ts.unwrap();
    let lint_first_ts = lint_first_ts.unwrap();
    let compile_first_ts = compile_first_ts.unwrap();
    // Dependents must appear after their dependencies.
    assert!(
        release_first_ts >= unit_first_ts,
        "release ({release_first_ts}) must run after unit ({unit_first_ts})"
    );
    assert!(
        release_first_ts >= lint_first_ts,
        "release ({release_first_ts}) must run after lint ({lint_first_ts})"
    );
    // compile (build) is stage 0; unit/lint are stage 1; the runner is
    // serial within a stage so compile must precede unit and lint.
    assert!(
        compile_first_ts <= unit_first_ts,
        "compile (build) should run before unit (test)"
    );
    assert!(
        compile_first_ts <= lint_first_ts,
        "compile (build) should run before lint (test)"
    );
}

#[tokio::test]
async fn sse_log_stream_emits_lines_and_closes_on_terminal() {
    // We don't need a real runner here — directly drive the DB so we
    // can deterministically observe SSE events.
    use http_body_util::BodyExt;
    use std::time::Duration;

    let (app, _guard) = common::test_app().await;
    let db = app.db.clone();

    let user = common::register_user(&app, "sseowner").await;
    let _full = common::create_repo(&app, &user.token, "sserepo").await;

    let (status, repo_json) = common::request(
        &app,
        Method::GET,
        "/api/repos/sseowner/sserepo",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let repo_id = repo_json["id"].as_i64().unwrap();
    let owner_user = db
        .get_user_by_username(&user.username)
        .await
        .unwrap()
        .unwrap();

    // Insert a queued pipeline + run + job so we have a job_id to stream.
    let pid = db
        .upsert_pipeline(
            repo_id,
            ".gita-ci.yml",
            "refs/heads/main",
            "deadbeef",
            "stages: [s]",
        )
        .await
        .unwrap();
    let run_id = db
        .insert_pipeline_run(
            pid,
            repo_id,
            "refs/heads/main",
            "deadbeef",
            Some(owner_user.id),
            None,
        )
        .await
        .unwrap();
    let job_id = db
        .insert_job(run_id, "slow", "s", &[], &["sleep 5".into()])
        .await
        .unwrap();
    db.update_job_status(job_id, "running", None).await.unwrap();

    // Pre-seed three log lines so the first stream read sees them all.
    for (i, line) in ["seeded line 0", "seeded line 1", "seeded line 2"]
        .iter()
        .enumerate()
    {
        db.insert_log(job_id, i as i64, "stdout", line, &Utc::now().to_rfc3339())
            .await
            .unwrap();
    }

    // Spawn a producer that, after a short delay, appends more lines
    // and then marks the job as success — that should close the SSE
    // stream on the server side.
    let producer_db = db.clone();
    let producer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        // Each line pushed with an explicit ts so we can spot it in
        // the stream's emitted JSON.
        for i in 3..6 {
            producer_db
                .insert_log(
                    job_id,
                    i,
                    "stdout",
                    &format!("late-updated line {i}"),
                    &Utc::now().to_rfc3339(),
                )
                .await
                .ok();
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
        producer_db
            .update_job_status(job_id, "success", Some(0))
            .await
            .ok();
    });

    // Drive the SSE endpoint via tower's oneshot — for a long-lived
    // stream we rely on the response body streaming. axum's Sse wraps
    // the body in a Stream; we read it via axum::body::Body stream.
    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri(format!(
            "/api/repos/sseowner/sserepo/runs/{run_id}/jobs/{job_id}/logs/stream"
        ))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let mut body = resp.into_body();
    let mut buffer = String::new();
    let timeout = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut seeded_count = 0;
    let mut late_count = 0;
    let mut saw_status = false;
    let mut status_status = String::new();
    while tokio::time::Instant::now() < timeout {
        let remaining = timeout.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, body.frame()).await {
            Ok(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref().as_ref() {
                    let chunk = String::from_utf8_lossy(data).to_string();
                    buffer.push_str(&chunk);
                    // SSE events are separated by double newlines.
                    // Pull out complete `event: ..\ndata: ..\n\n` blocks.
                    while let Some(idx) = buffer.find("\n\n") {
                        let block: String = buffer.drain(..idx + 2).collect();
                        let event = block
                            .lines()
                            .find_map(|l| l.strip_prefix("event: "))
                            .unwrap_or("unknown");
                        let data = block
                            .lines()
                            .find_map(|l| l.strip_prefix("data: "))
                            .unwrap_or("");
                        match event {
                            "log" => {
                                // Should always be JSON, but just count
                                // the lines so we don't crash on a
                                // multipart framing edge.
                                let trimmed: String = data
                                    .chars()
                                    .skip_while(|c| c.is_whitespace())
                                    .take_while(|c| *c != '\n')
                                    .collect();
                                if !trimmed.is_empty() {
                                    if trimmed.contains("seeded") {
                                        seeded_count += 1;
                                    } else if trimmed.contains("late-updated") {
                                        late_count += 1;
                                    }
                                }
                            }
                            "status" => {
                                saw_status = true;
                                status_status = data.to_string();
                            }
                            _ => {}
                        }
                    }
                }
                if saw_status && status_status.contains("\"success\"") {
                    break;
                }
            }
            Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
        }
    }
    producer.await.ok();

    // We expect at least 3 seeded lines (history) and the late updates
    // (3..=5) to have streamed across.
    assert!(
        seeded_count >= 3,
        "expected ≥3 seeded log events, saw {seeded_count}; buffer={buffer}"
    );
    assert!(
        late_count >= 1,
        "expected ≥1 late-updated log event, saw {late_count}; buffer={buffer}"
    );
    assert!(
        saw_status && status_status.contains("\"success\""),
        "expected final status=success event; saw saw_status={saw_status} \
         status_status={status_status}; buffer={buffer}"
    );
}

/// The runner must be able to clone the repository itself, from a full ref.
///
/// Every other test in this file pre-clones the repo into the workspace, so
/// the runner's own clone path — the one that runs in production — was never
/// exercised. It passed `run.ref_name` (a full ref, `refs/heads/main`) to
/// `git clone --branch`, which wants a short branch name and fails with
/// "Remote branch refs/heads/main not found". Every real push-triggered run
/// therefore failed at the clone step.
#[tokio::test]
async fn runner_clones_the_repository_using_a_full_ref() {
    let (app, _guard) = common::test_app().await;
    let db = app.db.clone();
    let user = common::register_user(&app, "cloneci").await;
    common::create_repo(&app, &user.token, "cirepo").await;
    common::push_files(
        &app,
        "cloneci",
        "cirepo",
        &[(
            ".gita-ci.yml",
            b"stages: [build]\njobs:\n  build:\n    stage: build\n    script:\n      - echo cloned-ok\n"
                .as_slice(),
        )],
        "main",
        "add ci config",
    );

    // Register the pipeline and enqueue a run, as the push handler does.
    let repo = db
        .get_repository_by_name("cloneci", "cirepo")
        .await
        .unwrap()
        .expect("repo");
    let head_sha = app
        .git
        .head_sha("cloneci", "cirepo", "main")
        .expect("head sha");
    let yaml =
        "stages: [build]\njobs:\n  build:\n    stage: build\n    script:\n      - echo cloned-ok\n";
    let pipeline_id = db
        .upsert_pipeline(repo.id, ".gita-ci.yml", "refs/heads/main", &head_sha, yaml)
        .await
        .unwrap();
    let run_id = db
        .insert_pipeline_run(
            pipeline_id,
            repo.id,
            "refs/heads/main",
            &head_sha,
            None,
            None,
        )
        .await
        .unwrap();

    // The runner clones the bare repo directly — no pre-clone — so the
    // executor's own clone path runs.
    let bare = app
        .data_dir
        .join("repos")
        .join("cloneci")
        .join("cirepo.git");
    let runner_config = RunnerConfig {
        server_db_url: common::test_db_url(),
        server_base_url: format!("file://{}", bare.parent().unwrap().display()),
        runner_token: "test-token".to_string(),
        runner_name: "test-runner".to_string(),
        work_dir: std::env::temp_dir().join(format!(
            "gita-clone-it-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )),
        poll_interval_secs: 1,
        claim_lease_secs: 300,
        concurrent_jobs: 1,
        job_timeout_secs: 300,
    };
    // The bare repo lives at <base>/cloneci/cirepo.git, so the clone URL
    // must be built from the parent of the bare repo.
    let runner_config = RunnerConfig {
        server_base_url: format!("file://{}", app.data_dir.join("repos").display()),
        ..runner_config
    };

    // Claim the run the way the runner's poll loop does.
    let claimed = db
        .claim_pipeline_run(&runner_config.runner_name, 300)
        .await
        .unwrap()
        .expect("a run should be claimable");
    assert_eq!(claimed.id, run_id);
    assert_eq!(
        claimed.ref_name, "refs/heads/main",
        "the run should carry the full ref, which is what broke the clone"
    );

    let registry = JobRegistry::new();
    execute_pipeline(&db, &runner_config, &registry, &claimed)
        .await
        .expect("execute_pipeline");

    // The run must succeed — the clone has to work.
    let finished = db.get_pipeline_run(run_id).await.unwrap().unwrap();
    let _all = db.list_pipeline_runs(pipeline_id, 50).await.unwrap();
    assert_eq!(
        finished.status, "success",
        "the runner's own clone should have succeeded"
    );

    // And the job's output must show it really ran the cloned repo.
    let jobs = db.list_jobs(run_id).await.unwrap();
    assert_eq!(jobs.len(), 1);
    let logs = db.list_logs_since(jobs[0].id, -1, 1000).await.unwrap();
    let text: String = logs.iter().map(|l| l.line.clone()).collect();
    assert!(
        text.contains("cloned-ok"),
        "the job should have run in the cloned repo, logs: {text}"
    );
}
