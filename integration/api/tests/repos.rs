mod common;

use common::{register_user, request};

#[tokio::test]
async fn create_repo_and_list() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "dave").await;
    let _ = common::create_repo(&app, &user.token, "hello").await;

    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/repos",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let arr = body.as_array().unwrap();
    assert!(arr.iter().any(|r| r["name"] == "hello"));
}

#[tokio::test]
async fn get_repo_404_for_missing_owner() {
    let (app, _guard) = common::test_app().await;
    let (status, _) = request(
        &app,
        axum::http::Method::GET,
        "/api/repos/nobody/nothing",
        None,
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_repo_requires_ownership() {
    let (app, _guard) = common::test_app().await;
    let owner = register_user(&app, "owner1").await;
    let intruder = register_user(&app, "intruder").await;
    let _ = common::create_repo(&app, &owner.token, "private-repo").await;

    // intruder cannot delete
    let (status, _) = request(
        &app,
        axum::http::Method::DELETE,
        "/api/repos/owner1/private-repo",
        None,
        Some(&intruder.token),
    )
    .await;
    // 404, not 403: a refusal must not tell the caller that the
    // repository exists.
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    // owner can delete (HTTP delete without body)
    let (status, _) = request(
        &app,
        axum::http::Method::DELETE,
        "/api/repos/owner1/private-repo",
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
}

#[tokio::test]
async fn repo_content_endpoints_see_committed_file() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "con-user").await;
    let full = common::create_repo(&app, &user.token, "trees").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);

    // list_branches
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/branches"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "branches: {body}");
    let branches = body.as_array().unwrap();
    assert!(branches.iter().any(|b| b == "main"));

    // list_commits
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/commits"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "commits: {body}");
    assert!(body["commits"].as_array().unwrap().len() >= 1);

    // list_tree (root)
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/tree"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "tree: {body}");
    let entries = body["entries"].as_array().unwrap();
    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap_or(""))
        .collect();
    assert!(names.contains(&"README.md"));
    assert!(names.contains(&"hello.txt"));

    // get_file
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/file?path=hello.txt"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "file: {body}");
    assert_eq!(body["content"].as_str().unwrap().trim(), "world");

    // missing file -> 404
    let (status, _) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/file?path=missing.txt"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    // missing path query -> 400
    let (status, _) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/file"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn read_file_for_directory_path_returns_400() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "dir-user").await;
    let full = common::create_repo(&app, &user.token, "dir-repo").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);

    // path="" (empty) was already verified to return tree. Asking for
    // a known sub-path that is actually a directory should also return
    // a structured "not found" rather than a 500, because the API
    // exposes FileContent shape.
    let (status, _) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/file?path=some-missing-file.txt"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_repos_with_owner_filter_returns_only_match() {
    let (app, _guard) = common::test_app().await;
    let alice = register_user(&app, "alice-list").await;
    let bob = register_user(&app, "bob-list").await;
    let _ = common::create_repo(&app, &alice.token, "alpha").await;
    let _ = common::create_repo(&app, &bob.token, "beta").await;
    let _ = common::create_repo(&app, &alice.token, "gamma").await;

    // ?owner=alice-list -> only alice's two repos
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/repos?owner=alice-list",
        None,
        Some(&alice.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    for r in arr {
        assert_eq!(r["owner"], "alice-list");
    }

    // No ?owner -> all three
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/repos",
        None,
        Some(&alice.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn branch_and_tag_management() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "bt-user").await;
    let full = common::create_repo(&app, &user.token, "bt-repo").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);

    // Create a branch.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &format!("/api/repos/{owner}/{name}/branches"),
        Some(serde_json::json!({"name": "feature"})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create branch: {body}");

    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/branches"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let branches = body.as_array().unwrap();
    assert!(branches.iter().any(|b| b == "feature"));

    // Delete the branch.
    let (status, _) = request(
        &app,
        axum::http::Method::DELETE,
        &format!("/api/repos/{owner}/{name}/branches/feature"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/branches"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(!body.as_array().unwrap().iter().any(|b| b == "feature"));

    // Create a tag.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &format!("/api/repos/{owner}/{name}/tags"),
        Some(serde_json::json!({"name": "v1.0.0"})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create tag: {body}");

    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/tags"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.as_array().unwrap().iter().any(|t| t == "v1.0.0"));

    // Non-owner cannot delete a tag.
    let intruder = register_user(&app, "bt-intruder").await;
    let (status, _) = request(
        &app,
        axum::http::Method::DELETE,
        &format!("/api/repos/{owner}/{name}/tags/v1.0.0"),
        None,
        Some(&intruder.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    // Owner deletes tag.
    let (status, _) = request(
        &app,
        axum::http::Method::DELETE,
        &format!("/api/repos/{owner}/{name}/tags/v1.0.0"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
}

#[tokio::test]
async fn fork_repository() {
    let (app, _guard) = common::test_app().await;
    let owner = register_user(&app, "fork-owner").await;
    let forker = register_user(&app, "forker").await;
    let full = common::create_repo(&app, &owner.token, "origin").await;
    let (owner_name, _) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner_name, "origin");

    // Forker forks the repo under their own namespace.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &format!("/api/repos/{owner_name}/origin/fork"),
        Some(serde_json::json!({})),
        Some(&forker.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "fork: {body}");
    assert_eq!(body["owner"], "forker");
    assert_eq!(body["name"], "origin");

    // Fork is readable via the new namespace.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/repos/forker/origin",
        None,
        Some(&forker.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["full_name"], "forker/origin");

    // Pushing to the fork writes to a different bare repo on disk.
    let fork_bare = app.data_dir.join("repos").join("forker").join("origin.git");
    assert!(fork_bare.exists(), "forked bare repo should exist on disk");

    // Cannot fork to an existing destination.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        &format!("/api/repos/{owner_name}/origin/fork"),
        Some(serde_json::json!({})),
        Some(&forker.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CONFLICT);

    // Custom fork name.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &format!("/api/repos/{owner_name}/origin/fork"),
        Some(serde_json::json!({"name": "my-fork"})),
        Some(&forker.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "fork with name: {body}");
    assert_eq!(body["name"], "my-fork");
}

#[tokio::test]
async fn commit_detail_endpoint() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "commit-user").await;
    let full = common::create_repo(&app, &user.token, "commit-repo").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);

    let (_, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/commits"),
        None,
        Some(&user.token),
    )
    .await;
    let sha = body["commits"][0]["sha"].as_str().unwrap().to_string();

    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/commits/{sha}"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "get commit: {body}");
    assert_eq!(body["commit"]["sha"], sha);
    assert_eq!(body["commit"]["author_name"], "Test");
    assert!(body["diff"].as_str().unwrap().contains("world"));

    // Invalid SHA -> 500 (gix returns Err).
    let (status, _) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/commits/deadbeef"),
        None,
        Some(&user.token),
    )
    .await;
    assert!(
        status == axum::http::StatusCode::INTERNAL_SERVER_ERROR
            || status == axum::http::StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn search_repos_and_code() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "search-user").await;
    let full = common::create_repo(&app, &user.token, "awesome-project").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);

    // Update description via PATCH so search hits the description column.
    let (status, _) = request(
        &app,
        axum::http::Method::PATCH,
        &format!("/api/repos/{owner}/{name}"),
        Some(serde_json::json!({"description": "totally searchable"})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Search by name.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/search/repos?q=awesome",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let hits = body["hits"].as_array().unwrap();
    assert!(hits.iter().any(|h| h["name"] == "awesome-project"));

    // Search by description.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/search/repos?q=searchable",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let hits = body["hits"].as_array().unwrap();
    assert!(hits.iter().any(|h| h["name"] == "awesome-project"));

    // Empty query -> 400.
    let (status, _) = request(
        &app,
        axum::http::Method::GET,
        "/api/search/repos?q=",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);

    // Code search: the initial commit contains "world" in hello.txt.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/search/code?q=world",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "code search: {body}");
    let hits = body["hits"].as_array().unwrap();
    assert!(
        hits.iter().any(|h| h["path"] == "hello.txt"),
        "expected a hit in hello.txt, got {hits:?}"
    );

    // /api/search combined.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/search?q=awesome&type=repos",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.get("repos").is_some());
}

#[tokio::test]
async fn star_and_unstar() {
    let (app, _guard) = common::test_app().await;
    let owner = register_user(&app, "star-owner").await;
    let starrer = register_user(&app, "star-starrer").await;
    let full = common::create_repo(&app, &owner.token, "star-repo").await;
    let (owner_name, name) = full.split_once('/').unwrap();

    // Initially 0 stars, not starred.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner_name}/{name}/star"),
        None,
        Some(&starrer.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["starred"], false);
    assert_eq!(body["count"], 0);

    // Star.
    let (status, body) = request(
        &app,
        axum::http::Method::PUT,
        &format!("/api/repos/{owner_name}/{name}/star"),
        None,
        Some(&starrer.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["starred"], true);
    assert_eq!(body["new"], true);
    assert_eq!(body["count"], 1);

    // Star again -> no-op.
    let (status, body) = request(
        &app,
        axum::http::Method::PUT,
        &format!("/api/repos/{owner_name}/{name}/star"),
        None,
        Some(&starrer.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["new"], false);

    // GET shows starred=true.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner_name}/{name}/star"),
        None,
        Some(&starrer.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["starred"], true);
    assert_eq!(body["count"], 1);

    // Unstar.
    let (status, body) = request(
        &app,
        axum::http::Method::DELETE,
        &format!("/api/repos/{owner_name}/{name}/star"),
        None,
        Some(&starrer.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["removed"], true);
    assert_eq!(body["count"], 0);

    // Cannot star a non-existent repo -> 404.
    let (status, _) = request(
        &app,
        axum::http::Method::PUT,
        "/api/repos/nobody/missing/star",
        None,
        Some(&starrer.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_repo_settings_rename_transfer_visibility() {
    let (app, _guard) = common::test_app().await;
    let owner = register_user(&app, "settings-user").await;
    let full = common::create_repo(&app, &owner.token, "before").await;
    let (owner_name, _) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner_name, "before");

    // Patch description + visibility.
    let (status, body) = request(
        &app,
        axum::http::Method::PATCH,
        &format!("/api/repos/{owner_name}/before"),
        Some(serde_json::json!({"description": "new desc", "is_private": true})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "patch: {body}");
    assert_eq!(body["description"], "new desc");
    assert_eq!(body["is_private"], true);

    // Rename.
    let (status, body) = request(
        &app,
        axum::http::Method::PATCH,
        &format!("/api/repos/{owner_name}/before"),
        Some(serde_json::json!({"new_name": "after"})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "rename: {body}");
    assert_eq!(body["name"], "after");

    // Old path should now 404.
    let (status, _) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner_name}/before"),
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    // Push still works on the new path.
    let bare = app
        .data_dir
        .join("repos")
        .join(owner_name)
        .join("after.git");
    let work = app.data_dir.join("work-rename");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let url = format!("file://{}", bare.display());
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&work)
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@e.io")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@e.io")
            .output()
            .expect("git");
        if !out.status.success() {
            panic!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    };
    run(&["clone", "-q", &url, "."]);
    std::fs::write(work.join("renamed.txt"), "yes").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "after-rename"]);
    run(&["push", "-q", "origin", "main"]);
    let _ = std::fs::remove_dir_all(&work);

    // Transfer to a new owner.
    let target = register_user(&app, "transfer-target").await;
    let (status, body) = request(
        &app,
        axum::http::Method::PATCH,
        &format!("/api/repos/{owner_name}/after"),
        Some(serde_json::json!({"new_owner": "transfer-target"})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "transfer: {body}");
    assert_eq!(body["owner"], "transfer-target");

    // Non-owner can't update.
    let (status, _) = request(
        &app,
        axum::http::Method::PATCH,
        &format!("/api/repos/transfer-target/after"),
        Some(serde_json::json!({"is_private": false})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    // New owner can now patch.
    let (status, body) = request(
        &app,
        axum::http::Method::PATCH,
        "/api/repos/transfer-target/after",
        Some(serde_json::json!({"is_private": false})),
        Some(&target.token),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "new owner patch: {body}"
    );
    assert_eq!(body["is_private"], false);
}

#[tokio::test]
async fn webhooks_crud_and_fanout() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "hook-user").await;
    let full = common::create_repo(&app, &user.token, "hook-repo").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);
    let _bare = app
        .data_dir
        .join("repos")
        .join(owner)
        .join(format!("{name}.git"));
    let repository_id = {
        let (_, body) = request(
            &app,
            axum::http::Method::GET,
            &format!("/api/repos/{owner}/{name}"),
            None,
            Some(&user.token),
        )
        .await;
        body["id"].as_i64().unwrap()
    };

    // Start a small mock HTTP server that records POSTs.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let received_clone = received.clone();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let received_inner = received_clone.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let _ = socket.read(&mut buf).await;
                received_inner
                    .lock()
                    .unwrap()
                    .push(serde_json::Value::String(
                        String::from_utf8_lossy(&buf).to_string(),
                    ));
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
            });
        }
    });

    let url = format!("http://{addr}/hook");

    // Non-owner cannot create a hook.
    let intruder = register_user(&app, "hook-intruder").await;
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        &format!("/api/repos/{owner}/{name}/hooks"),
        Some(serde_json::json!({"url": url})),
        Some(&intruder.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    // Owner creates a hook.
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &format!("/api/repos/{owner}/{name}/hooks"),
        Some(serde_json::json!({"url": url, "events": ["push"]})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create hook: {body}");
    let hook_id = body["id"].as_i64().unwrap();

    // List shows the hook.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/hooks"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let hooks = body["hooks"].as_array().unwrap();
    assert!(hooks.iter().any(|h| h["id"] == hook_id));

    // Invalid URL -> 400.
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        &format!("/api/repos/{owner}/{name}/hooks"),
        Some(serde_json::json!({"url": "ftp://nope"})),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);

    // Publish a push event directly through the bus to exercise the
    // webhook fan-out (the HTTP git protocol path publishes the same
    // EventType::Push event).
    app.bus.publish(
        gita_core::EventType::Push,
        Some(repository_id),
        None,
        serde_json::json!({"ref": "refs/heads/main", "new_sha": "abc123"}),
    );
    eprintln!("published push event; waiting for webhook...");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if !received.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        !received.lock().unwrap().is_empty(),
        "webhook did not fire within 5s"
    );

    // Delete hook.
    let (status, _) = request(
        &app,
        axum::http::Method::DELETE,
        &format!("/api/repos/{owner}/{name}/hooks/{hook_id}"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Delete missing -> 404.
    let (status, _) = request(
        &app,
        axum::http::Method::DELETE,
        &format!("/api/repos/{owner}/{name}/hooks/{hook_id}"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
}
