mod common;

use std::process::Command;

/// Push over real HTTP git and assert the receive-pack handler enqueues a CI
/// run from the pushed `.gita-ci.yml`.
///
/// The other tests in this file deliberately bypass the HTTP handler by
/// writing straight into the bare repo and inserting the pipeline via `Db`.
/// That leaves the handler's own CI trigger — which resolves the pushed
/// commit by its raw SHA — with no coverage at all.
///
/// That lookup is exactly where a bug lived: `read_file` treated any
/// non-`HEAD`/non-`refs/` reference as a branch name, so a raw commit SHA
/// became `refs/heads/<sha>` and silently resolved to nothing. The
/// `.gita-ci.yml` lookup therefore always returned `None` and **push-triggered
/// CI never ran**. This test is the regression guard for that.
#[tokio::test(flavor = "multi_thread")]
async fn http_push_with_ci_config_enqueues_run() {

    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "pushci").await;
    common::create_repo(&app, &user.token, "cirepo").await;

    // Serve the app's router over a real TCP listener so `git push` can talk
    // to it over HTTP rather than through `oneshot`.
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

    // The bare repo is the push target.
    let bare = app.data_dir.join("repos").join("pushci").join("cirepo.git");
    assert!(
        bare.exists(),
        "bare repo should exist at {}",
        bare.display()
    );
    let url = format!("http://127.0.0.1:{port}/pushci/cirepo.git");

    // Commit a `.gita-ci.yml` and push it over HTTP with credentials in the
    // URL — the git-CLI style the Basic-auth path expects.
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
    std::fs::write(
        work.join(".gita-ci.yml"),
        "stages:\n  - name: build\n    script:\n      - echo built\n",
    )
    .unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["remote", "add", "origin", &url]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "add ci config"]);

    let push_url = format!(
        "http://pushci:{}@127.0.0.1:{port}/pushci/cirepo.git",
        user.token
    );
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

    // The handler should have created the pipeline from the pushed commit.
    // Give the async event hook a moment to persist.
    let repo_id = app
        .db
        .get_repository_by_name("pushci", "cirepo")
        .await
        .unwrap()
        .expect("repo")
        .id;

    let mut pipeline = None;
    for _ in 0..50 {
        pipeline = app
            .db
            .get_pipeline_by_path(repo_id, ".gita-ci.yml")
            .await
            .unwrap();
        if pipeline.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let pipeline = pipeline.expect(
        "a push containing .gita-ci.yml should create a pipeline; if this fails, \
         check that the receive-pack handler resolves the pushed commit SHA",
    );
    assert!(
        pipeline.ref_name.contains("main"),
        "unexpected ref: {}",
        pipeline.ref_name
    );
    assert!(!pipeline.ref_sha.is_empty(), "head sha should be recorded");
}

/// A pipeline run must be linked to the event that triggered it.
///
/// `pipeline_runs.trigger_event_id` references `events(id)`, but it was
/// always NULL: the event was inserted by the bus's persist hook, which is
/// synchronous and cannot report the id it generated back to the caller
/// that needed it. The push handler now inserts the event itself and
/// backfills the link.
///
/// The multi-thread runtime flavour is required: the `git push` subprocess
/// blocks the calling thread while it runs, and the server task that has to
/// answer it is spawned onto the same runtime. On the default current-thread
/// runtime the two would deadlock — the push would wait for a response that
/// the server could never produce.
#[tokio::test(flavor = "multi_thread")]
async fn pipeline_run_is_linked_to_its_triggering_event() {
    let (app, _guard) = common::test_app().await;
    let schema = {
        // Read the schema name for the assertions below; the guard itself
        // must stay alive for the duration of the test.
        _guard.schema.clone()
    };
    let user = common::register_user(&app, "linkci2").await;
    common::create_repo(&app, &user.token, "cirepo").await;

    // Serve over a real TCP listener so `git push` can reach it.
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

    let url = format!("http://127.0.0.1:{port}/linkci2/cirepo.git");
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
    std::fs::write(
        work.join(".gita-ci.yml"),
        "stages: [build]\njobs:\n  build:\n    stage: build\n    script:\n      - echo built\n",
    )
    .unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["remote", "add", "origin", &url]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "add ci config"]);

    let push_url = format!(
        "http://linkci2:{}@127.0.0.1:{port}/linkci2/cirepo.git",
        user.token
    );
    let push = Command::new("git")
        .current_dir(&work)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "true")
        .args(["push", &push_url, "main:main"])
        .output()
        .expect("git push must run");
    assert!(
        push.status.success(),
        "git push failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&push.stdout),
        String::from_utf8_lossy(&push.stderr)
    );

    let repo_id = app
        .db
        .get_repository_by_name("linkci2", "cirepo")
        .await
        .unwrap()
        .expect("repo")
        .id;

    // Wait for the pipeline and its run to appear.
    let mut runs = Vec::new();
    for _ in 0..50 {
        if let Some(pipeline) = app
            .db
            .get_pipeline_by_path(repo_id, ".gita-ci.yml")
            .await
            .unwrap()
        {
            runs = app.db.list_pipeline_runs(pipeline.id, 10).await.unwrap();
            if !runs.is_empty() {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        !runs.is_empty(),
        "the push should have enqueued a run; pipeline present = {:?}",
        app.db
            .get_pipeline_by_path(repo_id, ".gita-ci.yml")
            .await
            .unwrap()
            .is_some()
    );

    let trigger_event_id = runs[0]
        .trigger_event_id
        .expect("the run must reference the event that triggered it");

    // The referenced id must point at a real `pipeline_requested` event,
    // and that event's payload must name this run.
    let pool = sqlx::PgPool::connect(&common::test_db_url()).await.unwrap();
    let row: (String, String) = sqlx::query_as(&format!(
        "SELECT event_type, payload FROM \"{schema}\".events WHERE id = $1"
    ))
    .bind(trigger_event_id)
    .fetch_one(&pool)
    .await
    .expect("the trigger event id must resolve to an event row");
    assert_eq!(row.0, "pipeline_requested");

    let payload: serde_json::Value = serde_json::from_str(&row.1).expect("event payload is JSON");
    assert_eq!(
        payload["run_id"].as_i64(),
        Some(runs[0].id),
        "the event payload must name the run it triggered"
    );

    // And the event must not have been inserted twice.
    let count: (i64,) = sqlx::query_as(&format!(
        "SELECT count(*) FROM \"{schema}\".events
         WHERE event_type = 'pipeline_requested' AND repo_id = $1"
    ))
    .bind(repo_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count.0, 1,
        "the event must be stored exactly once, found {} row(s)",
        count.0
    );
}

/// A push whose `.gita-ci.yml` uses the top-level job form (the format the
/// writing guide documents) must enqueue a run.
///
/// The parser only understood a `jobs:` mapping, so the guide's own Quick
/// start — and every example in it — produced a file that was rejected at
/// parse time, and the pipeline silently never ran.
#[tokio::test(flavor = "multi_thread")]
async fn http_push_with_top_level_ci_config_enqueues_run() {

    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "topci").await;
    common::create_repo(&app, &user.token, "cirepo").await;

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

    let url = format!("http://127.0.0.1:{port}/topci/cirepo.git");
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
    // The documented top-level job form, with a single-string `script:`
    // (also documented, and also previously rejected).
    std::fs::write(
        work.join(".gita-ci.yml"),
        "stages: [build]\n\nbuild:\n  stage: build\n  script: echo built\n",
    )
    .unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["remote", "add", "origin", &url]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "add ci config"]);

    let push_url = format!(
        "http://topci:{}@127.0.0.1:{port}/topci/cirepo.git",
        user.token
    );
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

    let repo_id = app
        .db
        .get_repository_by_name("topci", "cirepo")
        .await
        .unwrap()
        .expect("repo")
        .id;

    let mut pipeline = None;
    for _ in 0..50 {
        pipeline = app
            .db
            .get_pipeline_by_path(repo_id, ".gita-ci.yml")
            .await
            .unwrap();
        if pipeline.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let pipeline =
        pipeline.expect("a .gita-ci.yml in the documented top-level form should create a pipeline");
    assert!(
        pipeline.content.contains("echo built"),
        "unexpected content: {}",
        pipeline.content
    );
}

/// A path filter must be able to skip a run.
///
/// The push handler passed an empty changed-paths list to the trigger
/// evaluation, so `except: paths: [...]` could never match and a docs-only
/// commit still ran the full pipeline — even though the configuration asked
/// for exactly the opposite.
#[tokio::test(flavor = "multi_thread")]
async fn except_paths_filter_skips_a_docs_only_push() {

    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "pathfilter").await;
    common::create_repo(&app, &user.token, "cirepo").await;

    // A pipeline that only wants to run for source changes.
    common::push_files(
        &app,
        "pathfilter",
        "cirepo",
        &[(
            ".gita-ci.yml",
            b"stages: [build]\n\
              only:\n\
              \x20 branches: [main]\n\
              except:\n\
              \x20 paths: [\"docs/**\"]\n\
              jobs:\n\
              \x20 build:\n\
              \x20   stage: build\n\
              \x20   script:\n\
              \x20     - echo built\n"
                .as_slice(),
        )],
        "main",
        "add ci config",
    );

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

    let url = format!("http://127.0.0.1:{port}/pathfilter/cirepo.git");
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
    // A commit that only touches docs/.
    std::fs::create_dir_all(work.join("docs")).unwrap();
    std::fs::write(work.join("docs/readme.md"), "docs only\n").unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["remote", "add", "origin", &url]);
    // The .gita-ci.yml commit was already pushed by push_files; start from
    // it so this push is a docs-only change on top.
    git(&["pull", "-q", "--rebase", "origin", "main"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "docs only"]);

    let push_url = format!(
        "http://pathfilter:{}@127.0.0.1:{port}/pathfilter/cirepo.git",
        user.token
    );
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

    // No run may be enqueued: the only changed path is excluded. (The
    // pipeline *definition* row is still recorded — it reflects the repo's
    // CI config, which did change — but the run is what the filter gates.)
    let repo_id = app
        .db
        .get_repository_by_name("pathfilter", "cirepo")
        .await
        .unwrap()
        .expect("repo")
        .id;
    let mut runs = Vec::new();
    for _ in 0..30 {
        if let Some(pipeline) = app
            .db
            .get_pipeline_by_path(repo_id, ".gita-ci.yml")
            .await
            .unwrap()
        {
            runs = app.db.list_pipeline_runs(pipeline.id, 50).await.unwrap();
            if !runs.is_empty() {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        runs.is_empty(),
        "a docs-only push should have been filtered out by except.paths, \
         but a run was enqueued: {runs:?}"
    );
}

/// A branch filter written the way the guide documents must match.
///
/// The trigger evaluation compared the full ref (`refs/heads/main`) against
/// the branch globs, but a user writes `branches: [main, release/*]` — a
/// short name. Nothing ever matched, so `only: branches: [main]` silently
/// skipped every pipeline.
#[tokio::test(flavor = "multi_thread")]
async fn only_branches_filter_matches_a_short_branch_name() {

    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "branchfilter").await;
    common::create_repo(&app, &user.token, "cirepo").await;
    common::push_files(
        &app,
        "branchfilter",
        "cirepo",
        &[(
            ".gita-ci.yml",
            b"stages: [build]\n\
              only:\n\
              \x20 branches: [main]\n\
              jobs:\n\
              \x20 build:\n\
              \x20   stage: build\n\
              \x20   script:\n\
              \x20     - echo built\n"
                .as_slice(),
        )],
        "main",
        "add ci config",
    );

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

    let url = format!("http://127.0.0.1:{port}/branchfilter/cirepo.git");
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
    std::fs::write(work.join("app.txt"), "source\n").unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["remote", "add", "origin", &url]);
    git(&["pull", "-q", "--rebase", "origin", "main"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "source change"]);

    let push_url = format!(
        "http://branchfilter:{}@127.0.0.1:{port}/branchfilter/cirepo.git",
        user.token
    );
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

    // A run must be enqueued: the branch matches `only.branches: [main]`.
    let repo_id = app
        .db
        .get_repository_by_name("branchfilter", "cirepo")
        .await
        .unwrap()
        .expect("repo")
        .id;
    let mut runs = Vec::new();
    for _ in 0..30 {
        if let Some(pipeline) = app
            .db
            .get_pipeline_by_path(repo_id, ".gita-ci.yml")
            .await
            .unwrap()
        {
            runs = app.db.list_pipeline_runs(pipeline.id, 50).await.unwrap();
            if !runs.is_empty() {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        !runs.is_empty(),
        "a push to main must enqueue a run when only.branches is [main]"
    );
}
