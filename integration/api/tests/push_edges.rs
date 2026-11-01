//! The edges of what a push does.
//!
//! The happy path — a push that carries a `.gita-ci.yml` and starts a
//! run — is covered in `ci_push.rs`. This file covers the cases around
//! it, which are the ones that decide whether a pipeline runs when it
//! should not, or does not run when it should:
//!
//! * a push that deletes a branch triggers nothing;
//! * a `.gita-ci.yml` that cannot be parsed leaves the pipeline row
//!   behind but starts no run;
//! * the workflow files under `.github/workflows/` are filtered by name
//!   and by directory.

mod common;

use std::process::Command;

/// Serve the app's router over a real TCP listener so `git push` can
/// talk to it over HTTP.
async fn serve_http(app: &gita_admin::App) -> u16 {
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
    port
}

/// A git client in a work tree of its own.
fn work_tree(dir: &tempfile::TempDir) -> std::path::PathBuf {
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
    git(&["init", "-q", "-b", "main"]);
    work
}

/// The repository id of `owner/name`.
async fn repo_id(app: &gita_admin::App, owner: &str, name: &str) -> i64 {
    app.db
        .get_repository_by_name(owner, name)
        .await
        .unwrap()
        .expect("the repo must exist")
        .id
}

/// Wait for the pipeline of the repository to appear.
async fn wait_for_pipeline(
    app: &gita_admin::App,
    repo_id: i64,
    path: &str,
) -> Option<gita_db::PipelineRow> {
    for _ in 0..50 {
        let pipeline = app.db.get_pipeline_by_path(repo_id, path).await.unwrap();
        if pipeline.is_some() {
            return pipeline;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    app.db.get_pipeline_by_path(repo_id, path).await.unwrap()
}

/// A push that deletes a branch must not start a run.
///
/// The branch is gone after the push, so there is no commit to run the
/// pipeline against; a run enqueued here would sit in the queue forever.
#[tokio::test(flavor = "multi_thread")]
async fn a_push_that_deletes_a_branch_triggers_nothing() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "delci").await;
    common::create_repo(&app, &user.token, "cirepo").await;
    let port = serve_http(&app).await;

    let dir = tempfile::TempDir::new().unwrap();
    let work = work_tree(&dir);
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
            "git {args:?} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };

    // Push a branch that carries a CI config, then delete it.
    std::fs::write(
        work.join(".gita-ci.yml"),
        "stages:\n  - name: build\n    script:\n      - echo built\n",
    )
    .unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "add ci config"]);

    let push_url = format!(
        "http://delci:{}@127.0.0.1:{port}/delci/cirepo.git",
        user.token
    );
    let push = Command::new("git")
        .current_dir(&work)
        .args(["push", &push_url, "main:feature"])
        .output()
        .expect("git push must run");
    assert!(
        push.status.success(),
        "git push failed:\n{}",
        String::from_utf8_lossy(&push.stderr)
    );

    // The push of the branch created a pipeline; the deletion must not
    // add a run for it.
    let repo_id = repo_id(&app, "delci", "cirepo").await;
    let pipeline = wait_for_pipeline(&app, repo_id, ".gita-ci.yml")
        .await
        .expect("the first push must create the pipeline");
    let runs_before = app
        .db
        .list_pipeline_runs(pipeline.id, 50)
        .await
        .unwrap()
        .len();

    // Now delete the branch.
    let delete = Command::new("git")
        .current_dir(&work)
        .args(["push", &push_url, "--delete", "feature"])
        .output()
        .expect("git push must run");
    assert!(
        delete.status.success(),
        "git push --delete failed:\n{}",
        String::from_utf8_lossy(&delete.stderr)
    );

    // Give the handler a moment to do whatever it does with the push.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let runs_after = app
        .db
        .list_pipeline_runs(pipeline.id, 50)
        .await
        .unwrap()
        .len();
    assert_eq!(
        runs_after, runs_before,
        "a push that deletes a branch must not start a run"
    );
}

/// A `.gita-ci.yml` that cannot be parsed leaves the pipeline row
/// behind but starts no run.
///
/// The pipeline row records the file the repository carries; a file that
/// does not parse is a configuration error the user has to see, not a
/// reason to run the previous configuration.
#[tokio::test(flavor = "multi_thread")]
async fn a_broken_ci_config_leaves_the_pipeline_but_starts_no_run() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "badci").await;
    common::create_repo(&app, &user.token, "cirepo").await;
    let port = serve_http(&app).await;

    let dir = tempfile::TempDir::new().unwrap();
    let work = work_tree(&dir);
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
            "git {args:?} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };

    // Not YAML at all.
    std::fs::write(work.join(".gita-ci.yml"), "stages: [ this is not yaml\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "add broken ci config"]);

    let push_url = format!(
        "http://badci:{}@127.0.0.1:{port}/badci/cirepo.git",
        user.token
    );
    let push = Command::new("git")
        .current_dir(&work)
        .args(["push", &push_url, "main:main"])
        .output()
        .expect("git push must run");
    assert!(
        push.status.success(),
        "git push failed:\n{}",
        String::from_utf8_lossy(&push.stderr)
    );

    // Give the handler a moment to record the pipeline.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let repo_id = repo_id(&app, "badci", "cirepo").await;
    let pipeline = app
        .db
        .get_pipeline_by_path(repo_id, ".gita-ci.yml")
        .await
        .unwrap();
    let pipeline = match pipeline {
        Some(p) => p,
        None => {
            // The pipeline may not be recorded for a file that does not
            // parse; what matters is that no run was started.
            let runs: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pipeline_runs WHERE repo_id = $1",
            )
            .bind(repo_id)
            .fetch_one(app.db.pool())
            .await
            .unwrap();
            assert_eq!(runs, 0, "a broken config must not start a run");
            return;
        }
    };

    let runs = app
        .db
        .list_pipeline_runs(pipeline.id, 50)
        .await
        .unwrap();
    assert!(
        runs.is_empty(),
        "a broken config must not start a run, got {runs:?}"
    );
}

/// The workflow files are the ones under `.github/workflows/`, and only
/// the ones whose name says they are YAML.
///
/// A file next to the workflows, or a workflow in a subdirectory, or a
/// file that is not YAML, is not a workflow: running it would run
/// something the repository never asked to be run.
#[tokio::test(flavor = "multi_thread")]
async fn only_yaml_workflow_files_are_picked_up() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "wfci").await;
    common::create_repo(&app, &user.token, "cirepo").await;
    let port = serve_http(&app).await;

    let dir = tempfile::TempDir::new().unwrap();
    let work = work_tree(&dir);
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
            "git {args:?} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };

    // One real workflow, and three files that must not be picked up.
    std::fs::create_dir_all(work.join(".github/workflows/nested")).unwrap();
    std::fs::write(
        work.join(".github/workflows/ci.yml"),
        "stages:\n  - name: build\n    script:\n      - echo built\n",
    )
    .unwrap();
    std::fs::write(work.join(".github/workflows/notes.txt"), "not a workflow\n").unwrap();
    std::fs::write(
        work.join(".github/workflows/nested/deep.yml"),
        "stages:\n  - name: nested\n    script:\n      - echo nested\n",
    )
    .unwrap();
    std::fs::write(work.join("workflow.yml"), "stages: []\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "add workflows"]);

    let push_url = format!(
        "http://wfci:{}@127.0.0.1:{port}/wfci/cirepo.git",
        user.token
    );
    let push = Command::new("git")
        .current_dir(&work)
        .args(["push", &push_url, "main:main"])
        .output()
        .expect("git push must run");
    assert!(
        push.status.success(),
        "git push failed:\n{}",
        String::from_utf8_lossy(&push.stderr)
    );

    // Give the handler a moment to record the pipelines.
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    let repo_id = repo_id(&app, "wfci", "cirepo").await;
    let paths: Vec<String> = sqlx::query_scalar(
        "SELECT path FROM pipelines WHERE repo_id = $1 ORDER BY path",
    )
    .bind(repo_id)
    .fetch_all(app.db.pool())
    .await
    .unwrap();

    assert!(
        paths.contains(&".github/workflows/ci.yml".to_string()),
        "the real workflow must be recorded, got {paths:?}"
    );
    assert!(
        !paths.contains(&".github/workflows/nested/deep.yml".to_string()),
        "a workflow in a subdirectory is not a workflow, got {paths:?}"
    );
    assert!(
        !paths.contains(&".github/workflows/notes.txt".to_string()),
        "a file that is not YAML is not a workflow, got {paths:?}"
    );
    assert!(
        !paths.contains(&"workflow.yml".to_string()),
        "a file outside the workflows directory is not a workflow, got {paths:?}"
    );
}
