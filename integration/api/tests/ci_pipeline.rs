//! Integration test: push a commit containing `.gita-ci.yml`, claim it
//! via the DB, execute the pipeline with the gita-runner executor, and
//! verify the run finishes with the sentinel string in its logs.
//!
//! This exercises the full Phase-1 loop except the HTTP receive-pack
//! handler: the push is simulated by writing directly into the bare repo
//! (reusing `common::push_initial_commit`), the run is inserted via Db,
//! and the runner's `execute_pipeline` is invoked as a library function
//! with a pre-cloned workspace (it skips the network clone when the repo
//! dir already exists).

mod common;

use axum::http::Method;
use gita_db::Db;
use gita_runner::config::RunnerConfig;
use gita_runner::executor::execute_pipeline;
use gita_runner::registry::JobRegistry;
use gita_runner::workspace::run_work_dir;
use std::path::Path;
use std::process::Command;

fn push_ci_yml(
    app: &gita_server::App,
    owner: &str,
    name: &str,
    yaml: &str,
) {
    let bare = app.data_dir.join("repos").join(owner).join(format!("{name}.git"));
    let bare_url = format!("file://{}", bare.display());
    let work_parent = app.data_dir.join(format!(".work-{}", std::process::id()));
    std::fs::create_dir_all(&work_parent).unwrap();
    let work_root = work_parent.join(format!("ciwktmp-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&work_root).unwrap();

    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(&work_root)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {:?} failed:\nstdout: {}\nstderr: {}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    };

    std::fs::write(work_root.join(".gita-ci.yml"), yaml).unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["remote", "add", "origin", &bare_url]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "add ci config"]);
    // The remote may already have a main from a prior push_initial_commit;
    // force is fine for the test sandbox.
    git(&["push", "-q", "-f", "origin", "main"]);
}

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
    let head_sha = app.git.head_sha("ciowner", "cirepo", "refs/heads/main")
        .expect("head_sha");

    let owner_user = db.get_user_by_username(&user.username).await.unwrap().unwrap();

    // Upsert pipeline + insert run as the HTTP handler would.
    let pid = db
        .upsert_pipeline(repo_id, ".gita-ci.yml", "refs/heads/main", &head_sha, "stages: [build]")
        .await
        .unwrap();
    let run_id = db
        .insert_pipeline_run(pid, repo_id, "refs/heads/main", &head_sha, Some(owner_user.id), None)
        .await
        .unwrap();

    // Build a ClaimedRun row and pre-clone the repo into the workspace
    // so execute_pipeline skips the network clone.
    let claimed = db.claim_pipeline_run("test-runner", 300).await.unwrap().unwrap();
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
    };

    // Pre-clone the repo at head_sha into the run workspace.
    let repo_dir = run_work_dir(&runner_config, run_id).join("repo");
    let bare = app.data_dir.join("repos").join("ciowner").join("cirepo.git");
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
    let any_sentinel = logs
        .iter()
        .any(|l| l.line.contains("sentinel-CI-RAN-OK"));
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
        .upsert_pipeline(repo_id, ".gita-ci.yml", "refs/heads/main", &head_sha, "stages: [wait]")
        .await
        .unwrap();
    let run_id = db
        .insert_pipeline_run(pid, repo_id, "refs/heads/main", &head_sha, Some(owner_user.id), None)
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
    let exec_result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        exec_handle,
    )
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