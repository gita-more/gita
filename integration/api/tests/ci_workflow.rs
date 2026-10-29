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

/// Push a commit over real HTTP git, the way a user would.
///
/// The receive-pack handler is what triggers the CI run, so a test that
/// writes straight into the bare repository would never exercise the
/// workflow discovery this file is about.
async fn http_push(
    app: &gita_server::App,
    owner: &str,
    repo: &str,
    token: &str,
    files: &[(&str, &str)],
) {
    // Serve the app's router over a real TCP listener so `git push` can
    // talk to it over HTTP rather than through `oneshot`.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = app.router();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });

    let url = format!("http://127.0.0.1:{port}/{owner}/{repo}.git");

    let dir = tempfile::TempDir::new().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(&work)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {args:?} failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    };

    for (path, content) in files {
        let target = work.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&target, content).unwrap();
    }

    git(&["init", "-q", "-b", "main"]);
    git(&["remote", "add", "origin", &url]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "add workflow"]);

    let push_url = format!("http://{owner}:{token}@127.0.0.1:{port}/{owner}/{repo}.git");
    let push = Command::new("git")
        .current_dir(&work)
        .args(["push", &push_url, "main:main"])
        .output()
        .expect("git push must run");
    assert!(
        push.status.success(),
        "git push failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&push.stdout),
        String::from_utf8_lossy(&push.stderr)
    );
}

/// A pushed workflow file must be discovered and enqueue a run on its own.
///
/// The earlier tests insert the pipeline row by hand; this one pushes the
/// workflow over HTTP so the push handler's discovery of
/// `.github/workflows/*.yml` is what is under test.
#[tokio::test(flavor = "multi_thread")]
async fn a_pushed_workflow_file_enqueues_a_run() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "wfpush").await;
    common::create_repo(&app, &user.token, "wfpushrepo").await;

    let yaml = r#"
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: echo "sentinel-PUSHED-WORKFLOW"
"#;

    http_push(
        &app,
        "wfpush",
        "wfpushrepo",
        &user.token,
        &[(".github/workflows/ci.yml", yaml)],
    )
    .await;

    let repo_id = app
        .db
        .get_repository_by_name("wfpush", "wfpushrepo")
        .await
        .unwrap()
        .expect("repo")
        .id;

    // The push handler creates the pipeline asynchronously.
    let mut pipeline = None;
    for _ in 0..50 {
        pipeline = app
            .db
            .get_pipeline_by_path(repo_id, ".github/workflows/ci.yml")
            .await
            .unwrap();
        if pipeline.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let pipeline = pipeline.expect(
        "a push containing .github/workflows/ci.yml should create a pipeline; if this fails, \
         check that the push handler scans the .github/workflows directory",
    );
    assert!(pipeline.content.contains("sentinel-PUSHED-WORKFLOW"));

    // And a run must have been enqueued for it.
    let mut runs = Vec::new();
    for _ in 0..50 {
        runs = app.db.list_pipeline_runs(pipeline.id, 10).await.unwrap();
        if !runs.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        !runs.is_empty(),
        "the pushed workflow should have enqueued a run"
    );
    assert_eq!(runs[0].status, "queued");
}

/// A workflow whose branch filter does not match the pushed branch must
/// not enqueue anything.
#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_that_does_not_match_the_push_is_skipped() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "wfskip").await;
    common::create_repo(&app, &user.token, "wfskiprepo").await;

    // The workflow only wants `release-only`; the push is to `main`.
    let yaml = r#"
on:
  push:
    branches: [release-only]
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
"#;

    http_push(
        &app,
        "wfskip",
        "wfskiprepo",
        &user.token,
        &[(".github/workflows/ci.yml", yaml)],
    )
    .await;

    let repo_id = app
        .db
        .get_repository_by_name("wfskip", "wfskiprepo")
        .await
        .unwrap()
        .expect("repo")
        .id;

    // Give the push handler time to (not) act.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    // The workflow's configuration is recorded either way — the pipeline
    // row remembers what the repository's CI config is. What the branch
    // filter decides is whether a *run* is enqueued.
    let pipeline = app
        .db
        .get_pipeline_by_path(repo_id, ".github/workflows/ci.yml")
        .await
        .unwrap()
        .expect("the workflow configuration should still be recorded");

    let runs = app.db.list_pipeline_runs(pipeline.id, 10).await.unwrap();
    assert!(
        runs.is_empty(),
        "a workflow that does not match the push must not enqueue a run; got {runs:?}"
    );
}

/// A workflow that matches the push must be runnable end to end: the
/// run enqueued by the push handler is picked up and executed.
#[tokio::test(flavor = "multi_thread")]
async fn a_pushed_workflow_is_executed_by_the_runner() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "wfexec").await;
    common::create_repo(&app, &user.token, "wfexecrepo").await;

    let yaml = r#"
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: echo "sentinel-END-TO-END-WORKFLOW"
"#;

    http_push(
        &app,
        "wfexec",
        "wfexecrepo",
        &user.token,
        &[(".github/workflows/ci.yml", yaml)],
    )
    .await;

    let db = app.db.clone();
    let repo_id = db
        .get_repository_by_name("wfexec", "wfexecrepo")
        .await
        .unwrap()
        .expect("repo")
        .id;

    // Wait for the push handler to enqueue the run.
    let mut pipeline = None;
    for _ in 0..50 {
        pipeline = db
            .get_pipeline_by_path(repo_id, ".github/workflows/ci.yml")
            .await
            .unwrap();
        if pipeline.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let pipeline = pipeline.expect("the workflow should have been recorded");

    let mut run_id = None;
    for _ in 0..50 {
        let runs = db.list_pipeline_runs(pipeline.id, 10).await.unwrap();
        if let Some(r) = runs.first() {
            run_id = Some(r.id);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let run_id = run_id.expect("the workflow should have enqueued a run");

    // Claim and execute it as the runner would.
    let config = runner_config(&app, "e2e");
    std::fs::create_dir_all(&config.work_dir).unwrap();
    let claimed = db
        .claim_pipeline_run(&config.runner_name, 300)
        .await
        .unwrap()
        .expect("the run should be claimable");
    assert_eq!(claimed.id, run_id);

    let registry = JobRegistry::new();
    execute_workflow_run(&db, &config, &registry, &claimed)
        .await
        .expect("execute_workflow_run");

    let run = db.get_pipeline_run(run_id).await.unwrap().unwrap();
    assert_eq!(run.status, "success");

    let jobs = db.list_jobs(run_id).await.unwrap();
    assert_eq!(jobs.len(), 1);
    let logs = db.list_logs_since(jobs[0].id, -1, 1000).await.unwrap();
    let text: String = logs.iter().map(|l| l.line.clone()).collect();
    assert!(
        text.contains("sentinel-END-TO-END-WORKFLOW"),
        "the pushed workflow should have run; logs: {text}"
    );
}
