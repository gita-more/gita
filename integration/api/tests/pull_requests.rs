mod common;

use common::{register_user, request};

#[tokio::test]
async fn pull_request_full_lifecycle() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "pr-user").await;
    let full = common::create_repo(&app, &user.token, "pr-repo").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);

    // create PR (source_branch=main, target_branch=main -> should succeed even with same)
    // the API does NOT require head != base — only that head_sha and base_sha exist.
    let uri_prs = format!("/api/repos/{owner}/{name}/prs");
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &uri_prs,
        Some(serde_json::json!({
            "title": "feat: hello world",
            "body": "first PR",
            "source_branch": "main",
            "target_branch": "main",
        })),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create_pr: {body}");
    let pr_number = body["number"].as_i64().unwrap();
    assert_eq!(body["status"], "open");

    // list_prs (default: open)
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &uri_prs,
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["prs"].as_array().unwrap().len(), 1);

    // get_pr
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("{uri_prs}/{pr_number}"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["pr"]["number"], pr_number);

    // list_prs with status filter
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("{uri_prs}?status=closed"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["prs"].as_array().unwrap().len(), 0);

    // close_pr (must succeed even though target=head; close is allowed)
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &format!("{uri_prs}/{pr_number}/close"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "close: {body}");

    // re-close -> 409 (only open PRs can be closed)
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        &format!("{uri_prs}/{pr_number}/close"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
}

#[tokio::test]
async fn pull_request_invalid_status_filter_rejected() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "pr-user2").await;
    let full = common::create_repo(&app, &user.token, "filter-repo").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);

    let (status, _) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/prs?status=bogus"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pull_request_comments_lifecycle() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "cmt-pr-user").await;
    let full = common::create_repo(&app, &user.token, "cmt-pr").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);

    // Create a PR.
    let uri_prs = format!("/api/repos/{owner}/{name}/prs");
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &uri_prs,
        Some(serde_json::json!({
            "title": "comment me",
            "body": "body",
            "source_branch": "main",
            "target_branch": "main",
        })),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create pr: {body}");
    let pr_number = body["number"].as_i64().unwrap();

    let uri_comments = format!("/api/repos/{owner}/{name}/prs/{pr_number}/comments");

    // Initially empty.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &uri_comments,
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["comments"].as_array().unwrap().len(), 0);

    // Add a comment.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &uri_comments,
        Some(serde_json::json!({"body": "looks good"})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create comment: {body}");
    let comment_id = body["id"].as_i64().unwrap();
    assert_eq!(body["author"], "cmt-pr-user");
    assert_eq!(body["body"], "looks good");

    // Now list -> 1.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &uri_comments,
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["comments"].as_array().unwrap().len(), 1);

    // Empty body -> 400.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        &uri_comments,
        Some(serde_json::json!({"body": "  "})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);

    // Delete the comment.
    let (status, _) = request(
        &app,
        axum::http::Method::DELETE,
        &format!("/api/comments/{comment_id}"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "delete comment");

    // List -> 0 again.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &uri_comments,
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["comments"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn pull_request_merge_conflict_detected_and_rejected() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "conflict-user").await;
    let full = common::create_repo(&app, &user.token, "conflict-repo").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);

    let bare = app
        .data_dir
        .join("repos")
        .join(owner)
        .join(format!("{name}.git"));
    let bare_url = format!("file://{}", bare.display());

    // Helper to run git in a temp worktree.
    fn git_in(work: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(work)
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@e.io")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@e.io")
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let work_parent = app.data_dir.join("work-conflict");
    std::fs::create_dir_all(&work_parent).unwrap();
    let work = work_parent.join("wt");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();

    // Clone main, create feature branch that changes README.md.
    git_in(&work, &["clone", "-q", "-b", "main", &bare_url, "."]);
    std::fs::write(work.join("README.md"), "# feature version\n").unwrap();
    git_in(&work, &["checkout", "-q", "-b", "feature"]);
    git_in(&work, &["add", "."]);
    git_in(&work, &["commit", "-q", "-m", "feature readme"]);
    git_in(&work, &["push", "-q", "-u", "origin", "feature"]);

    // On main, change README.md differently and push.
    git_in(&work, &["checkout", "-q", "main"]);
    std::fs::write(work.join("README.md"), "# main version\n").unwrap();
    git_in(&work, &["add", "."]);
    git_in(&work, &["commit", "-q", "-m", "main readme"]);
    git_in(&work, &["push", "-q", "origin", "main"]);

    let uri_prs = format!("/api/repos/{owner}/{name}/prs");
    // Create PR from feature -> main.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &uri_prs,
        Some(serde_json::json!({
            "title": "conflicting PR",
            "body": "",
            "source_branch": "feature",
            "target_branch": "main",
        })),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create pr: {body}");
    let pr_number = body["number"].as_i64().unwrap();

    // get_pr should report conflicts.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/prs/{pr_number}"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "get pr: {body}");
    let conflicts = body["pr"]["conflicts"].as_array().unwrap();
    assert!(
        conflicts.iter().any(|c| c == "README.md"),
        "expected README.md conflict, got {conflicts:?}"
    );

    // merge should fail with 409 conflict.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        &format!("/api/repos/{owner}/{name}/prs/{pr_number}/merge"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::CONFLICT,
        "merge should conflict"
    );

    let _ = std::fs::remove_dir_all(&work_parent);
}

#[tokio::test]
async fn pull_request_non_ff_merge_creates_merge_commit() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "merge-user").await;
    let full = common::create_repo(&app, &user.token, "merge-repo").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);

    let bare = app
        .data_dir
        .join("repos")
        .join(owner)
        .join(format!("{name}.git"));
    let bare_url = format!("file://{}", bare.display());

    fn git_in(work: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(work)
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@e.io")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@e.io")
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let work_parent = app.data_dir.join("work-merge");
    std::fs::create_dir_all(&work_parent).unwrap();
    let work = work_parent.join("wt");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();

    // Feature branch adds a file (no conflict with main).
    git_in(&work, &["clone", "-q", "-b", "main", &bare_url, "."]);
    std::fs::write(work.join("feature.txt"), "feature\n").unwrap();
    git_in(&work, &["checkout", "-q", "-b", "feature"]);
    git_in(&work, &["add", "."]);
    git_in(&work, &["commit", "-q", "-m", "feature work"]);
    git_in(&work, &["push", "-q", "-u", "origin", "feature"]);

    // Main adds a different file.
    git_in(&work, &["checkout", "-q", "main"]);
    std::fs::write(work.join("main.txt"), "main\n").unwrap();
    git_in(&work, &["add", "."]);
    git_in(&work, &["commit", "-q", "-m", "main work"]);
    git_in(&work, &["push", "-q", "origin", "main"]);

    let uri_prs = format!("/api/repos/{owner}/{name}/prs");
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &uri_prs,
        Some(serde_json::json!({
            "title": "mergeable PR",
            "body": "",
            "source_branch": "feature",
            "target_branch": "main",
        })),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create pr: {body}");
    let pr_number = body["number"].as_i64().unwrap();

    // No conflicts.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/prs/{pr_number}"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["pr"]["conflicts"].as_array().unwrap().len(), 0);

    // Merge succeeds and returns a merge_sha different from feature head.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &format!("/api/repos/{owner}/{name}/prs/{pr_number}/merge"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "merge: {body}");
    let merge_sha = body["merge_sha"].as_str().unwrap().to_string();
    assert!(!merge_sha.is_empty());

    let feature_sha = {
        let out = std::process::Command::new("git")
            .args([
                "--git-dir",
                bare.to_str().unwrap(),
                "rev-parse",
                "refs/heads/feature",
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    // A real merge commit should differ from the feature head.
    assert_ne!(
        merge_sha, feature_sha,
        "merge commit should differ from feature head"
    );

    let _ = std::fs::remove_dir_all(&work_parent);
}
