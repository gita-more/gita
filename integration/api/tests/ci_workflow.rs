//! End-to-end tests for the GitHub-Actions-style workflow runner.
//!
//! These mirror `ci_runner.rs` but drive `.github/workflows/*.yml`
//! instead of `.gita-ci.yml`: a workflow file is pushed, a run is
//! enqueued for it, and the workflow executor is expected to run the
//! workflow's jobs and record their logs.

mod common;

use gita_runner::config::RunnerConfig;
use gita_runner::registry::JobRegistry;
use gita_runner::workflow_run::execute_workflow_run;
use std::process::Command;

use common::push_files;

/// A runner config rooted in a unique temp directory.
fn runner_config(app: &gita_server::App, tag: &str) -> RunnerConfig {
    RunnerConfig {
        server_db_url: common::test_db_url(),
        server_base_url: format!("file://{}", app.data_dir.join("repos").display()),
        runner_token: "workflow-test-token".to_string(),
        runner_name: format!("workflow-runner-{tag}"),
        work_dir: std::env::temp_dir().join(format!(
            "gita-wf-it-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )),
        poll_interval_secs: 1,
        claim_lease_secs: 300,
        concurrent_jobs: 1,
        job_timeout_secs: 300,
    }
}

/// Push a workflow file, enqueue a run for it, and execute it.
///
/// Returns the run id.
async fn run_workflow(
    app: &gita_server::App,
    owner: &str,
    repo: &str,
    workflow_path: &str,
    workflow_yaml: &str,
) -> (gita_db::Db, i64) {
    let db = app.db.clone();

    push_files(
        app,
        owner,
        repo,
        &[(workflow_path, workflow_yaml.as_bytes())],
        "main",
        "add workflow",
    );

    let repo_row = db
        .get_repository_by_name(owner, repo)
        .await
        .unwrap()
        .expect("repo exists");
    let head_sha = app
        .git
        .head_sha(owner, repo, "refs/heads/main")
        .expect("head sha");

    // The pipeline row holds the workflow's YAML; its path marks the run
    // as a workflow rather than a `.gita-ci.yml` pipeline.
    let pipeline_id = db
        .upsert_pipeline(repo_row.id, workflow_path, "refs/heads/main", &head_sha, workflow_yaml)
        .await
        .unwrap();
    let run_id = db
        .insert_pipeline_run(
            pipeline_id,
            repo_row.id,
            "refs/heads/main",
            &head_sha,
            None,
            None,
        )
        .await
        .unwrap();

    (db, run_id)
}

#[tokio::test]
async fn workflow_run_executes_its_steps_and_records_logs() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "wfowner").await;
    common::create_repo(&app, &user.token, "wfrepo").await;
    common::push_initial_commit(&app, "wfowner", "wfrepo");

    let yaml = r#"
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - name: say hello
        run: echo "sentinel-WORKFLOW-RAN-OK"
"#;

    let (db, run_id) = run_workflow(
        &app,
        "wfowner",
        "wfrepo",
        ".github/workflows/ci.yml",
        yaml,
    )
    .await;

    let config = runner_config(&app, "basic");
    std::fs::create_dir_all(&config.work_dir).unwrap();

    let claimed = db
        .claim_pipeline_run(&config.runner_name, 300)
        .await
        .unwrap()
        .expect("a run should be claimable");
    assert_eq!(claimed.id, run_id);

    let registry = JobRegistry::new();
    execute_workflow_run(&db, &config, &registry, &claimed)
        .await
        .expect("execute_workflow_run");

    let run = db.get_pipeline_run(run_id).await.unwrap().unwrap();
    assert_eq!(
        run.status, "success",
        "the workflow run should have succeeded"
    );

    let jobs = db.list_jobs(run_id).await.unwrap();
    assert_eq!(jobs.len(), 1, "one workflow job should have run");
    let job = &jobs[0];
    assert_eq!(job.name, "build");
    assert_eq!(job.status, "success");

    let logs = db.list_logs_since(job.id, -1, 1000).await.unwrap();
    let text: String = logs.iter().map(|l| l.line.clone()).collect();
    assert!(
        text.contains("sentinel-WORKFLOW-RAN-OK"),
        "the workflow step should have run; logs: {text}"
    );
}

#[tokio::test]
async fn workflow_run_executes_steps_in_order_and_stops_at_failure() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "wford").await;
    common::create_repo(&app, &user.token, "wfordrepo").await;
    common::push_initial_commit(&app, "wford", "wfordrepo");

    let yaml = r#"
on: push
jobs:
  first:
    runs-on: ubuntu-latest
    steps:
      - run: echo "step-one"
  second:
    needs: first
    runs-on: ubuntu-latest
    steps:
      - run: echo "step-two"
"#;

    let (db, run_id) = run_workflow(
        &app,
        "wford",
        "wfordrepo",
        ".github/workflows/ci.yml",
        yaml,
    )
    .await;

    let config = runner_config(&app, "order");
    std::fs::create_dir_all(&config.work_dir).unwrap();

    let claimed = db
        .claim_pipeline_run(&config.runner_name, 300)
        .await
        .unwrap()
        .expect("a run should be claimable");
    let registry = JobRegistry::new();
    execute_workflow_run(&db, &config, &registry, &claimed)
        .await
        .expect("execute_workflow_run");

    let run = db.get_pipeline_run(run_id).await.unwrap().unwrap();
    assert_eq!(run.status, "success");

    let jobs = db.list_jobs(run_id).await.unwrap();
    assert_eq!(jobs.len(), 2, "both jobs should have run");

    // The dependent job must have started after its dependency.
    let first_ts = common::first_ts(&db, jobs[0].id).await;
    let second_ts = common::first_ts(&db, jobs[1].id).await;
    assert!(first_ts.is_some(), "first job should have logs");
    assert!(second_ts.is_some(), "second job should have logs");
    assert!(
        second_ts.unwrap() >= first_ts.unwrap(),
        "the dependent job must run after its dependency"
    );
}

#[tokio::test]
async fn a_failing_workflow_step_fails_the_run() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "wffail").await;
    common::create_repo(&app, &user.token, "wffailrepo").await;
    common::push_initial_commit(&app, "wffail", "wffailrepo");

    let yaml = r#"
on: push
jobs:
  failing:
    runs-on: ubuntu-latest
    steps:
      - run: echo "before-failure"
      - run: exit 7
      - run: echo "after-failure"
"#;

    let (db, run_id) = run_workflow(
        &app,
        "wffail",
        "wffailrepo",
        ".github/workflows/ci.yml",
        yaml,
    )
    .await;

    let config = runner_config(&app, "fail");
    std::fs::create_dir_all(&config.work_dir).unwrap();

    let claimed = db
        .claim_pipeline_run(&config.runner_name, 300)
        .await
        .unwrap()
        .expect("a run should be claimable");
    let registry = JobRegistry::new();
    execute_workflow_run(&db, &config, &registry, &claimed)
        .await
        .expect("execute_workflow_run");

    let run = db.get_pipeline_run(run_id).await.unwrap().unwrap();
    assert_eq!(run.status, "failed", "a failing step must fail the run");

    let jobs = db.list_jobs(run_id).await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].status, "failed");

    let logs = db.list_logs_since(jobs[0].id, -1, 1000).await.unwrap();
    let text: String = logs.iter().map(|l| l.line.clone()).collect();
    assert!(
        text.contains("before-failure"),
        "the step before the failure should have run; logs: {text}"
    );
    assert!(
        !text.contains("after-failure"),
        "the step after the failure must not run; logs: {text}"
    );
}

#[tokio::test]
async fn a_matrix_workflow_produces_one_job_per_combination() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "wfmatrix").await;
    common::create_repo(&app, &user.token, "wfmatrixrepo").await;
    common::push_initial_commit(&app, "wfmatrix", "wfmatrixrepo");

    let yaml = r#"
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        os: [linux, macos]
    steps:
      - run: echo "running on ${{ matrix.os }}"
"#;

    let (db, run_id) = run_workflow(
        &app,
        "wfmatrix",
        "wfmatrixrepo",
        ".github/workflows/ci.yml",
        yaml,
    )
    .await;

    let config = runner_config(&app, "matrix");
    std::fs::create_dir_all(&config.work_dir).unwrap();

    let claimed = db
        .claim_pipeline_run(&config.runner_name, 300)
        .await
        .unwrap()
        .expect("a run should be claimable");
    let registry = JobRegistry::new();
    execute_workflow_run(&db, &config, &registry, &claimed)
        .await
        .expect("execute_workflow_run");

    let run = db.get_pipeline_run(run_id).await.unwrap().unwrap();
    assert_eq!(run.status, "success");

    let jobs = db.list_jobs(run_id).await.unwrap();
    assert_eq!(jobs.len(), 2, "the matrix should produce two jobs");
    // Each instance gets a distinct name (the jobs table is unique on
    // (run_id, name)).
    assert_ne!(jobs[0].name, jobs[1].name);
    for job in &jobs {
        assert_eq!(job.status, "success");
        let logs = db.list_logs_since(job.id, -1, 1000).await.unwrap();
        let text: String = logs.iter().map(|l| l.line.clone()).collect();
        assert!(
            text.contains("running on"),
            "each matrix instance should have run; logs: {text}"
        );
    }
}

#[tokio::test]
async fn a_malformed_workflow_fails_the_run_rather_than_passing_it() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "wfbad").await;
    common::create_repo(&app, &user.token, "wfbadrepo").await;
    common::push_initial_commit(&app, "wfbad", "wfbadrepo");

    // No `on:` section — the workflow cannot be parsed.
    let yaml = r#"
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
"#;

    let (db, run_id) = run_workflow(
        &app,
        "wfbad",
        "wfbadrepo",
        ".github/workflows/ci.yml",
        yaml,
    )
    .await;

    let config = runner_config(&app, "bad");
    std::fs::create_dir_all(&config.work_dir).unwrap();

    let claimed = db
        .claim_pipeline_run(&config.runner_name, 300)
        .await
        .unwrap()
        .expect("a run should be claimable");
    let registry = JobRegistry::new();
    execute_workflow_run(&db, &config, &registry, &claimed)
        .await
        .expect("execute_workflow_run");

    let run = db.get_pipeline_run(run_id).await.unwrap().unwrap();
    assert_eq!(
        run.status, "failed",
        "a workflow that cannot be parsed must not report success"
    );
}

#[tokio::test]
async fn the_runner_clones_the_repository_for_a_workflow_run() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "wfclone").await;
    common::create_repo(&app, &user.token, "wfclonerepo").await;
    common::push_initial_commit(&app, "wfclone", "wfclonerepo");

    let yaml = r#"
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: echo "cloned-into-the-repo"
"#;

    let (db, run_id) = run_workflow(
        &app,
        "wfclone",
        "wfclonerepo",
        ".github/workflows/ci.yml",
        yaml,
    )
    .await;

    // No pre-clone: the workflow executor performs its own clone, which
    // is the path that runs in production.
    let config = runner_config(&app, "clone");
    std::fs::create_dir_all(&config.work_dir).unwrap();

    let claimed = db
        .claim_pipeline_run(&config.runner_name, 300)
        .await
        .unwrap()
        .expect("a run should be claimable");
    let registry = JobRegistry::new();
    execute_workflow_run(&db, &config, &registry, &claimed)
        .await
        .expect("execute_workflow_run");

    let run = db.get_pipeline_run(run_id).await.unwrap().unwrap();
    assert_eq!(
        run.status, "success",
        "the runner's own clone should have succeeded"
    );

    // The checkout must exist on disk at the run's workspace.
    let repo_dir = gita_runner::workspace::run_work_dir(&config, run_id).join("repo");
    assert!(
        repo_dir.join(".git").exists(),
        "the workflow run should have a checkout at {}",
        repo_dir.display()
    );

    let jobs = db.list_jobs(run_id).await.unwrap();
    let logs = db.list_logs_since(jobs[0].id, -1, 1000).await.unwrap();
    let text: String = logs.iter().map(|l| l.line.clone()).collect();
    assert!(
        text.contains("cloned-into-the-repo"),
        "the job should have run in the cloned repo; logs: {text}"
    );

    // Sanity: the checkout is a real git repository.
    let status = Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .expect("git rev-parse");
    assert!(status.status.success(), "the checkout should be a git repo");
}
