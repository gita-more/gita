mod common;

use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn e2e_repo_clone() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "carol");

    // Create repo via binary.
    gita_ok(port, &token, &["repo", "create", "clone-demo"]);

    let tmp = TempDir::new().unwrap();
    let dest = tmp.path().join("clone-demo");
    let out = gita_ok(
        port,
        &token,
        &[
            "repo",
            "clone",
            "carol/clone-demo",
            "--dir",
            dest.to_str().unwrap(),
        ],
    );
    stdout_contains(&out, "Cloned");

    // The cloned dir should exist and be a git repo.
    assert!(dest.join(".git").is_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_repo_with_commit() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "dev1");

    // Create repo.
    gita_ok(port, &token, &["repo", "create", "myrepo"]);

    // Push an initial commit to the default branch.
    push_initial_commit(port, &token, "dev1", "dev1", "myrepo", "main");

    // Tree should show the pushed files.
    let out = gita_ok(port, &token, &["repo", "tree", "dev1", "myrepo"]);
    stdout_contains(&out, "README.md");
    stdout_contains(&out, "src/main.rs");

    // Branches should include main.
    let out = gita_ok(port, &token, &["repo", "branches", "dev1", "myrepo"]);
    stdout_contains(&out, "main");

    // Commits should include the initial commit message.
    let out = gita_ok(port, &token, &["repo", "commits", "dev1", "myrepo"]);
    stdout_contains(&out, "Initial commit");

    // File content should be readable.
    let out = gita_ok(
        port,
        &token,
        &["repo", "file", "dev1", "myrepo", "README.md"],
    );
    stdout_contains(&out, "# Hello");

    // Commit view should show the SHA and message.
    let commits: serde_json::Value =
        gita_json(port, &token, &["repo", "commits", "dev1", "myrepo"]);
    let commits_arr = commits["commits"].as_array().expect("commits array");
    assert!(!commits_arr.is_empty(), "expected at least one commit");
    let sha = commits_arr[0]["sha"].as_str().expect("commit sha");
    let out = gita_ok(port, &token, &["repo", "commit", "dev1", "myrepo", sha]);
    stdout_contains(&out, sha);
    stdout_contains(&out, "Initial commit");
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_repo_star_unstar() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "dev4");

    // Create a repo.
    gita_ok(port, &token, &["repo", "create", "stardemo"]);

    // Star it.
    let out = gita_ok(port, &token, &["repo", "star", "dev4", "stardemo"]);
    stdout_contains(&out, "starred: true");

    // Unstar it.
    let out = gita_ok(port, &token, &["repo", "unstar", "dev4", "stardemo"]);
    stdout_contains(&out, "starred: false");
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_repo_fork() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let alice_token = register_via_binary(port, "alice-fork");
    let bob_token = register_via_binary(port, "bob-fork");

    // Alice creates a repo.
    gita_ok(port, &alice_token, &["repo", "create", "original"]);

    // Bob forks alice-fork/original.
    let out = gita_ok(
        port,
        &bob_token,
        &[
            "repo",
            "fork",
            "alice-fork",
            "original",
            "--name",
            "original-copy",
        ],
    );
    stdout_contains(&out, "original-copy");

    // Bob's repo list should include the fork.
    let out = gita_ok(port, &bob_token, &["repo", "list"]);
    stdout_contains(&out, "original-copy");
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_repo_edit() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "editor");

    // Create a repo.
    gita_ok(port, &token, &["repo", "create", "editme"]);

    // Edit description.
    let out = gita_ok(
        port,
        &token,
        &[
            "repo",
            "edit",
            "editor",
            "editme",
            "--description",
            "Updated desc",
        ],
    );
    stdout_contains(&out, "Updated desc");

    // Make private.
    let out = gita_ok(
        port,
        &token,
        &["repo", "edit", "editor", "editme", "--private"],
    );
    stdout_contains(&out, "private: true");

    // Rename.
    let out = gita_ok(
        port,
        &token,
        &["repo", "edit", "editor", "editme", "--name", "renamed-repo"],
    );
    stdout_contains(&out, "renamed-repo");

    // Confirm the new name appears in the repo list.
    let out = gita_ok(port, &token, &["repo", "list"]);
    stdout_contains(&out, "renamed-repo");
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_repo_branch_tag() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "brtag");

    // Create a repo and push an initial commit so branches/tags have a base.
    gita_ok(port, &token, &["repo", "create", "brdemo"]);
    push_initial_commit(port, &token, "brtag", "brtag", "brdemo", "main");

    // Create a branch.
    let out = gita_ok(
        port,
        &token,
        &["repo", "branch-create", "brtag", "brdemo", "feature"],
    );
    stdout_contains(&out, "feature");

    // List branches — should include it.
    let out = gita_ok(port, &token, &["repo", "branches", "brtag", "brdemo"]);
    stdout_contains(&out, "feature");

    // Delete the branch.
    let out = gita_ok(
        port,
        &token,
        &["repo", "branch-delete", "brtag", "brdemo", "feature"],
    );
    stdout_contains(&out, "deleted");

    // Branches list should no longer contain it.
    let out = gita_ok(port, &token, &["repo", "branches", "brtag", "brdemo"]);
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("feature"),
        "feature branch should be removed"
    );

    // Create a tag.
    let out = gita_ok(
        port,
        &token,
        &["repo", "tag-create", "brtag", "brdemo", "v1.0"],
    );
    stdout_contains(&out, "v1.0");

    // List tags — should include it.
    let out = gita_ok(port, &token, &["repo", "tags", "brtag", "brdemo"]);
    stdout_contains(&out, "v1.0");

    // Delete the tag.
    let out = gita_ok(
        port,
        &token,
        &["repo", "tag-delete", "brtag", "brdemo", "v1.0"],
    );
    stdout_contains(&out, "deleted");

    // Tags list should no longer contain it.
    let out = gita_ok(port, &token, &["repo", "tags", "brtag", "brdemo"]);
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("v1.0"),
        "v1.0 tag should be removed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_repo_star_status() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "starstatus");

    // Create a repo.
    gita_ok(port, &token, &["repo", "create", "ssdemo"]);

    // Not starred initially.
    let out = gita_ok(
        port,
        &token,
        &["repo", "star-status", "starstatus", "ssdemo"],
    );
    stdout_contains(&out, "starred: false");

    // Star it, then status should flip.
    gita_ok(port, &token, &["repo", "star", "starstatus", "ssdemo"]);
    let out = gita_ok(
        port,
        &token,
        &["repo", "star-status", "starstatus", "ssdemo"],
    );
    stdout_contains(&out, "starred: true");

    // Unstar, status flips back.
    gita_ok(port, &token, &["repo", "unstar", "starstatus", "ssdemo"]);
    let out = gita_ok(
        port,
        &token,
        &["repo", "star-status", "starstatus", "ssdemo"],
    );
    stdout_contains(&out, "starred: false");
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_repo_pages() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "pagesuser");

    // Create a repo and push so a branch exists for Pages.
    gita_ok(port, &token, &["repo", "create", "sitedemo"]);
    push_initial_commit(port, &token, "pagesuser", "pagesuser", "sitedemo", "main");

    // Enable Pages.
    let out = gita_ok(
        port,
        &token,
        &["repo", "pages-enable", "pagesuser", "sitedemo"],
    );
    stdout_contains(&out, "enabled");

    // Status should report enabled.
    let out = gita_ok(
        port,
        &token,
        &["repo", "pages-status", "pagesuser", "sitedemo"],
    );
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap_or_default()
    });
    assert_eq!(status["enabled"], true);

    // Show config.
    let out = gita_ok(port, &token, &["repo", "pages", "pagesuser", "sitedemo"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("enabled"),
        "pages show should contain config"
    );

    // Disable Pages.
    let out = gita_ok(
        port,
        &token,
        &["repo", "pages-disable", "pagesuser", "sitedemo"],
    );
    stdout_contains(&out, "disabled");
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_repo_hooks() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "hookuser");

    // Create a repo.
    gita_ok(port, &token, &["repo", "create", "hookdemo"]);

    // Create a webhook.
    let out = gita_ok(
        port,
        &token,
        &[
            "repo",
            "hook-create",
            "hookuser",
            "hookdemo",
            "http://example.com/hook",
            "--events",
            "push,issues",
        ],
    );
    stdout_contains(&out, "Created hook #1");

    // List hooks — should include the URL.
    let out = gita_ok(port, &token, &["repo", "hooks", "hookuser", "hookdemo"]);
    stdout_contains(&out, "http://example.com/hook");

    // Delete the hook.
    let out = gita_ok(
        port,
        &token,
        &["repo", "hook-delete", "hookuser", "hookdemo", "1"],
    );
    stdout_contains(&out, "deleted");

    // List hooks again — should be empty.
    let out = gita_ok(port, &token, &["repo", "hooks", "hookuser", "hookdemo"]);
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("example.com"),
        "hook should be removed"
    );
}
