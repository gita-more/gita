mod common;

use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn e2e_pr_full_flow() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "alice");

    // Create repo and push initial commit to main.
    gita_ok(port, &token, &["repo", "create", "prdemo"]);
    push_initial_commit(port, &token, "alice", "alice", "prdemo", "main");

    // Push a second commit to a feature branch.
    push_commit_to_branch(
        port,
        &token,
        "alice",
        "alice",
        "prdemo",
        "feature",
        "src/feature.rs",
        "fn feature() {}\n",
        "Add feature",
    );

    // Create PR: feature → main.
    let out = gita_ok(
        port,
        &token,
        &[
            "pr",
            "create",
            "alice",
            "prdemo",
            "--title",
            "Feature PR",
            "--body",
            "Adds the new feature",
            "--source-branch",
            "feature",
            "--target-branch",
            "main",
        ],
    );
    stdout_contains(&out, "Created PR #1");

    // List PRs — should include the new PR.
    let out = gita_ok(port, &token, &["pr", "list", "alice", "prdemo"]);
    stdout_contains(&out, "Feature PR");

    // View PR #1.
    let out = gita_ok(port, &token, &["pr", "view", "alice", "prdemo", "1"]);
    stdout_contains(&out, "PR #1");
    stdout_contains(&out, "Feature PR");
    stdout_contains(&out, "feature");
    stdout_contains(&out, "main");

    // PR diff should be available.
    let out = gita_ok(port, &token, &["pr", "diff", "alice", "prdemo", "1"]);
    assert!(
        !String::from_utf8_lossy(&out.stdout).is_empty(),
        "PR diff should not be empty"
    );

    // Merge PR #1.
    let out = gita_ok(port, &token, &["pr", "merge", "alice", "prdemo", "1"]);
    stdout_contains(&out, "merged:");
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_issue_full_flow() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "alice");

    // Create repo.
    gita_ok(port, &token, &["repo", "create", "issuedemo"]);

    // Create issue.
    let out = gita_ok(
        port,
        &token,
        &[
            "issue",
            "create",
            "alice",
            "issuedemo",
            "--title",
            "Bug report",
            "--body",
            "Something is broken",
        ],
    );
    stdout_contains(&out, "Created issue #1");

    // List issues — should include the new issue.
    let out = gita_ok(port, &token, &["issue", "list", "alice", "issuedemo"]);
    stdout_contains(&out, "Bug report");

    // View issue #1.
    let out = gita_ok(port, &token, &["issue", "view", "alice", "issuedemo", "1"]);
    stdout_contains(&out, "Issue #1");
    stdout_contains(&out, "Bug report");

    // Add a comment.
    let out = gita_ok(
        port,
        &token,
        &[
            "issue",
            "comment",
            "alice",
            "issuedemo",
            "1",
            "--body",
            "Looking into it",
        ],
    );
    stdout_contains(&out, "Commented on issue #1");

    // Close issue #1.
    let out = gita_ok(port, &token, &["issue", "close", "alice", "issuedemo", "1"]);
    stdout_contains(&out, "closed");
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_issue_global_list() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "globissue");

    // Create two repos and an issue in each.
    gita_ok(port, &token, &["repo", "create", "repo-a"]);
    gita_ok(port, &token, &["repo", "create", "repo-b"]);
    gita_ok(
        port,
        &token,
        &[
            "issue",
            "create",
            "globissue",
            "repo-a",
            "--title",
            "Issue in A",
        ],
    );
    gita_ok(
        port,
        &token,
        &[
            "issue",
            "create",
            "globissue",
            "repo-b",
            "--title",
            "Issue in B",
        ],
    );

    // Global list should show both, with repo names.
    let out = gita_ok(port, &token, &["issue", "list", "--all"]);
    stdout_contains(&out, "Issue in A");
    stdout_contains(&out, "Issue in B");
    stdout_contains(&out, "globissue/repo-a");
    stdout_contains(&out, "globissue/repo-b");
}
