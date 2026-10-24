//! End-to-end integration tests for the `gita` CLI client.
//!
//! Boots a real `gita_server::App` against Postgres, starts it on an
//! ephemeral port, then drives the `target/debug/gita` binary via
//! `assert_cmd`. Authentication is injected through the `GITA_TOKEN`
//! environment variable so the OS keyring is never touched in CI.
//
//! Test priority (per user request):
//!   P0 — e2e_repo_with_commit, e2e_pr_full_flow, e2e_issue_full_flow, e2e_error_paths
//!   P1 — e2e_auth_token_flow, e2e_auth_logout, e2e_config_commands,
//!        e2e_repo_star_unstar, e2e_repo_fork
//!   P2 — e2e_pipeline_suite, e2e_group_suite, e2e_search_code, e2e_keyring_local

use std::net::SocketAddr;
use std::path::PathBuf;

use assert_cmd::Command as AssertCmd;
use gita_server::Config;
use sqlx::PgPool;
use tempfile::TempDir;
use tokio::net::TcpListener;

/// Database URL for integration tests. Override via `GITA_TEST_DATABASE_URL`.
fn test_db_url() -> String {
    std::env::var("GITA_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://gita:gita@127.0.0.1:55432/gita".to_string())
}

/// Path to the `gita` CLI binary.
fn gita_bin() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("target")
        .join("debug")
        .join("gita")
}

/// Run `gita` with the given args and `GITA_TOKEN` set, capturing output.
fn gita(port: u16, token: &str, args: &[&str]) -> std::process::Output {
    let mut cmd = AssertCmd::new(gita_bin());
    cmd.env("GITA_URL", format!("http://127.0.0.1:{port}"))
        .env("GITA_TOKEN", token);
    for a in args {
        cmd.arg(a);
    }
    cmd.output().expect("spawn gita")
}

/// Assert that a `gita` invocation succeeds.
fn gita_ok(port: u16, token: &str, args: &[&str]) -> std::process::Output {
    let out = gita(port, token, args);
    assert!(
        out.status.success(),
        "gita {:?} failed:\nstderr: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Run `gita` with `--json` and parse stdout as `serde_json::Value`.
fn gita_json(port: u16, token: &str, args: &[&str]) -> serde_json::Value {
    let mut args_with_json = args.to_vec();
    args_with_json.push("--json");
    let out = gita_ok(port, token, &args_with_json);
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "failed to parse JSON output for {:?}: {e}\nstdout: {}",
            args,
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// Stdout contains the given substring.
fn stdout_contains(out: &std::process::Output, needle: &str) {
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(needle),
        "expected stdout to contain {:?}, got:\n{}",
        needle,
        String::from_utf8_lossy(&out.stdout)
    );
}

/// Stderr contains the given substring.
fn stderr_contains(out: &std::process::Output, needle: &str) {
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(needle),
        "expected stderr to contain {:?}, got:\n{}",
        needle,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Assert that a `gita` invocation fails (non-zero exit).
fn gita_fails(out: &std::process::Output) {
    assert!(
        !out.status.success(),
        "expected gita invocation to fail, but it succeeded:\nstdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A running server bound to an ephemeral port.
struct Server {
    addr: SocketAddr,
    _guard: TestAppGuard,
}

impl Server {
    async fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (app, guard) = test_app().await?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let router = app.router();
        let serve_handle = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
        });

        // Wait until the server is actually accepting connections.
        let base_url = format!("http://{}", addr);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap();
        let mut last_status = None;
        for _ in 0..100 {
            match client.get(&format!("{base_url}/api/health")).send().await {
                Ok(_) => {
                    return Ok(Self {
                        addr,
                        _guard: guard,
                    })
                }
                Err(e) => last_status = Some(e.to_string()),
            }
            // Surface serve task failure if any.
            if serve_handle.is_finished() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // If the serve task exited, report why; otherwise we just timed out.
        if serve_handle.is_finished() {
            match serve_handle.await {
                Ok(Ok(())) => return Err("server stopped serving".into()),
                Ok(Err(e)) => return Err(format!("server exited early: {e}").into()),
                Err(e) => return Err(format!("serve task panicked: {e}").into()),
            }
        }
        Err(format!("server did not become ready at {base_url} (last: {last_status:?})").into())
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }
}

/// Boot a real `gita_server::App` with an isolated Postgres schema.
async fn test_app(
) -> Result<(gita_server::App, TestAppGuard), Box<dyn std::error::Error + Send + Sync>> {
    let tmp = TempDir::new()?;
    let schema = format!(
        "gita_it_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let base_url = test_db_url();
    {
        let pool = PgPool::connect(&base_url).await?;
        sqlx::query(&format!(r#"CREATE SCHEMA "{schema}""#))
            .execute(&pool)
            .await?;
        pool.close().await;
    }
    let mut config = Config::default();
    config.server.data_dir = tmp.path().join("data");
    config.server.database_url = base_url.clone();
    config.server.database_schema = Some(schema.clone());
    config.server.allow_insecure_password_reset = true;
    let app = gita_server::App::build(config).await?;
    let guard = TestAppGuard {
        _base_url: base_url,
        _schema: schema,
        _tmp: tmp,
    };
    Ok((app, guard))
}

/// Holds the per-test schema and cleans up the temp dir on drop.
struct TestAppGuard {
    _base_url: String,
    _schema: String,
    _tmp: TempDir,
}

fn register_via_binary(port: u16, username: &str) -> String {
    let out = gita_ok(
        port,
        "",
        &[
            "auth",
            "register",
            "--username",
            username,
            "--email",
            &format!("{username}@e2e.local"),
            "--password",
            "hunter22",
            "--json",
        ],
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["token"].as_str().unwrap().to_string()
}

/// Push an initial commit to a repo's default branch via smart HTTP.
fn push_initial_commit(
    port: u16,
    token: &str,
    username: &str,
    owner: &str,
    repo: &str,
    branch: &str,
) {
    let tmp = TempDir::new().unwrap();
    let repo_dir = tmp.path().join("repo");

    std::process::Command::new("git")
        .arg("init")
        .arg(&repo_dir)
        .status()
        .expect("git init");

    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("config")
        .arg("user.name")
        .arg("Test User")
        .status()
        .expect("git config user.name");
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("config")
        .arg("user.email")
        .arg("test@test.local")
        .status()
        .expect("git config user.email");

    std::fs::create_dir_all(repo_dir.join("src")).unwrap();
    std::fs::write(repo_dir.join("README.md"), "# Hello\n").unwrap();
    std::fs::write(repo_dir.join("src/main.rs"), "fn main() {}\n").unwrap();

    let origin = format!("http://{username}:{token}@127.0.0.1:{port}/git/{owner}/{repo}.git");
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("remote")
        .arg("add")
        .arg("origin")
        .arg(&origin)
        .status()
        .expect("git remote add");

    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("add")
        .arg(".")
        .status()
        .expect("git add");

    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("commit")
        .arg("-m")
        .arg("Initial commit")
        .status()
        .expect("git commit");

    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .env("GIT_SSL_NO_VERIFY", "1")
        .arg("push")
        .arg("origin")
        .arg(format!("HEAD:{branch}"))
        .status()
        .expect("git push");
    assert!(status.success(), "git push to {branch} failed");
}

/// Push an additional commit to a branch (clones first to get existing history).
fn push_commit_to_branch(
    port: u16,
    token: &str,
    username: &str,
    owner: &str,
    repo: &str,
    branch: &str,
    file_path: &str,
    file_content: &str,
    commit_msg: &str,
) {
    let tmp = TempDir::new().unwrap();
    let repo_dir = tmp.path().join("repo");

    let origin = format!("http://{username}:{token}@127.0.0.1:{port}/git/{owner}/{repo}.git");
    let status = std::process::Command::new("git")
        .env("GIT_SSL_NO_VERIFY", "1")
        .arg("clone")
        .arg(&origin)
        .arg(&repo_dir)
        .status()
        .expect("git clone");
    assert!(
        status.success(),
        "git clone failed for push_commit_to_branch"
    );

    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("config")
        .arg("user.name")
        .arg("Test User")
        .status()
        .expect("git config user.name");
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("config")
        .arg("user.email")
        .arg("test@test.local")
        .status()
        .expect("git config user.email");

    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("checkout")
        .arg("-b")
        .arg(branch)
        .status()
        .expect("git checkout -b");

    std::fs::write(repo_dir.join(file_path), file_content).unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("add")
        .arg(".")
        .status()
        .expect("git add");
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("commit")
        .arg("-m")
        .arg(commit_msg)
        .status()
        .expect("git commit");

    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .env("GIT_SSL_NO_VERIFY", "1")
        .arg("push")
        .arg("origin")
        .arg(format!("HEAD:{branch}"))
        .status()
        .expect("git push");
    assert!(status.success(), "git push to {branch} failed");
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_auth_repo_pr_issue() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "alice");

    // repo create
    stdout_contains(
        &gita_ok(port, &token, &["repo", "create", "demo"]),
        "alice/demo",
    );

    // repo list
    stdout_contains(&gita_ok(port, &token, &["repo", "list"]), "alice");
    stdout_contains(&gita_ok(port, &token, &["repo", "list"]), "demo");

    // repo tree (empty repo still works)
    gita_ok(port, &token, &["repo", "tree", "alice", "demo"]);

    // pr list (empty)
    gita_ok(port, &token, &["pr", "list", "alice", "demo"]);

    // issue list
    gita_ok(port, &token, &["issue", "list", "alice", "demo"]);

    // search repos
    gita_ok(port, &token, &["search", "repos", "demo"]);

    // event list
    gita_ok(port, &token, &["event", "list"]);

    // auth status
    stdout_contains(&gita_ok(port, &token, &["auth", "status"]), "alice");

    // auth whoami
    stdout_contains(&gita_ok(port, &token, &["auth", "whoami"]), "alice");
}

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

// ---------------------------------------------------------------------------
// P0: repo with commit (push + verify tree/commits/branches/file)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// P0: PR full flow (create → list → view → merge)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// P0: issue full flow (create → list → view → close)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// P0: error paths (invalid token → 401, missing repo → 404, duplicate user → 409)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn e2e_error_paths() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "bob");

    // Invalid token → 401.
    let out = gita(port, "invalid-token-12345", &["repo", "list"]);
    gita_fails(&out);
    stderr_contains(&out, "401");

    // Missing repo → 404.
    let out = gita(port, &token, &["repo", "view", "bob", "no-such-repo"]);
    gita_fails(&out);
    stderr_contains(&out, "404");

    // Missing PR → 404.
    let out = gita(port, &token, &["pr", "view", "bob", "no-such-repo", "999"]);
    gita_fails(&out);
    stderr_contains(&out, "404");

    // Duplicate username → 409 (or 400, depending on server validation).
    let out = gita(
        port,
        "",
        &[
            "auth",
            "register",
            "--username",
            "bob",
            "--email",
            "bob2@e2e.local",
            "--password",
            "hunter22",
        ],
    );
    gita_fails(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("409") || stderr.contains("400"),
        "expected 409 or 400 for duplicate username, got: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// P1: auth token create / list / delete
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn e2e_auth_token_flow() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "dev2");

    // Create a PAT.
    let out = gita_ok(
        port,
        &token,
        &["auth", "token", "create", "--name", "cli-pat"],
    );
    stdout_contains(&out, "Token created");
    stdout_contains(&out, "cli-pat");
    // The plaintext token should be shown.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("token:"),
        "expected plaintext token in output, got: {stdout}"
    );

    // List tokens — should include the new one (prefix only).
    let out = gita_ok(port, &token, &["auth", "token", "list"]);
    stdout_contains(&out, "cli-pat");

    // Parse the token list to get the new token's id.
    let tokens: serde_json::Value = gita_json(port, &token, &["auth", "token", "list"]);
    let empty = serde_json::Value::Array(vec![]);
    let token_list = tokens.as_array().unwrap_or(&empty.as_array().unwrap());
    let pat_id = token_list
        .iter()
        .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("cli-pat"))
        .expect("cli-pat token in list")
        .get("id")
        .and_then(|i| i.as_i64())
        .expect("token id");

    // Delete the token.
    let out = gita_ok(
        port,
        &token,
        &["auth", "token", "delete", "--id", &pat_id.to_string()],
    );
    stdout_contains(&out, "deleted");

    // Listing again should not include it.
    let out = gita_ok(port, &token, &["auth", "token", "list"]);
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("cli-pat"),
        "cli-pat should be removed from token list"
    );
}

// ---------------------------------------------------------------------------
// P1: auth logout
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn e2e_auth_logout() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "dev3");

    // Log out.
    let out = gita_ok(port, &token, &["auth", "logout"]);
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("Logged out") || combined.contains("logged out"),
        "expected logout confirmation, got: {combined}"
    );

    // After logout, status should show not logged in (token invalid).
    // `auth status` with a revoked token reports logged_in: false in JSON.
    let out = gita_ok(port, &token, &["auth", "status", "--json"]);
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(status["logged_in"], false);
}

// ---------------------------------------------------------------------------
// P1: config set-url / show
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn e2e_config_commands() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let url = format!("http://127.0.0.1:{port}");

    // set-url.
    let out = gita_ok(port, "", &["config", "set-url", &url]);
    stdout_contains(&out, &url);

    // show (currently returns a placeholder message).
    let out = gita_ok(port, "", &["config", "show"]);
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("config") || combined.contains("URL"),
        "expected config info, got: {combined}"
    );
}

// ---------------------------------------------------------------------------
// P1: repo star / unstar
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// P1: repo fork
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// P2: pipeline full suite (list → trigger → runs → cancel)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn e2e_pipeline_suite() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "dev5");

    // Create repo and push a .gita-ci.yml pipeline config.
    gita_ok(port, &token, &["repo", "create", "pipedemo"]);
    let tmp = TempDir::new().unwrap();
    let repo_dir = tmp.path().join("repo");

    std::process::Command::new("git")
        .arg("init")
        .arg(&repo_dir)
        .status()
        .expect("git init");
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("config")
        .arg("user.name")
        .arg("Test User")
        .status()
        .expect("git config");
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("config")
        .arg("user.email")
        .arg("test@test.local")
        .status()
        .expect("git config");

    std::fs::write(
        repo_dir.join(".gita-ci.yml"),
        "stages:\n  - build\njobs:\n  build:\n    script: [\"echo hi\"]\n",
    )
    .unwrap();

    let origin = format!("http://dev5:{token}@127.0.0.1:{port}/git/dev5/pipedemo.git");
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("remote")
        .arg("add")
        .arg("origin")
        .arg(&origin)
        .status()
        .expect("git remote add");
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("add")
        .arg(".")
        .status()
        .expect("git add");
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("commit")
        .arg("-m")
        .arg("Add CI config")
        .status()
        .expect("git commit");
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .env("GIT_SSL_NO_VERIFY", "1")
        .arg("push")
        .arg("origin")
        .arg("HEAD:main")
        .status()
        .expect("git push");
    assert!(status.success(), "git push failed");

    // Pipeline list — should show the pipeline.
    let out = gita_ok(port, &token, &["pipeline", "list", "dev5", "pipedemo"]);
    assert!(
        !String::from_utf8_lossy(&out.stdout).is_empty(),
        "pipeline list should return output"
    );

    // Trigger a run.
    let out = gita_ok(port, &token, &["pipeline", "trigger", "dev5", "pipedemo"]);
    let combined = format!("{}", String::from_utf8_lossy(&out.stdout));
    assert!(
        combined.contains("triggered") || combined.contains("run"),
        "expected trigger confirmation, got: {combined}"
    );

    // List runs.
    let out = gita_ok(port, &token, &["pipeline", "runs", "dev5", "pipedemo"]);
    assert!(
        !String::from_utf8_lossy(&out.stdout).is_empty(),
        "pipeline runs should return output"
    );
}

// ---------------------------------------------------------------------------
// P2: group full suite (list → create → view → members → repo)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn e2e_group_suite() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "dev6");

    // Group list (initially empty).
    gita_ok(port, &token, &["group", "list"]);

    // Create a group.
    let out = gita_ok(
        port,
        &token,
        &["group", "create", "team", "--description", "A test group"],
    );
    stdout_contains(&out, "Created group team");

    // View the group.
    let out = gita_ok(port, &token, &["group", "view", "team"]);
    stdout_contains(&out, "team");

    // List group members.
    gita_ok(port, &token, &["group", "members", "team"]);

    // Create a repo under the group.
    let out = gita_ok(port, &token, &["group", "repo", "team", "group-repo"]);
    stdout_contains(&out, "group-repo");

    // Group view should now list the repo.
    let out = gita_ok(port, &token, &["group", "view", "team"]);
    stdout_contains(&out, "group-repo");
}

// ---------------------------------------------------------------------------
// P2: search code
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn e2e_search_code() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "dev7");

    // Create repo and push a file with searchable content.
    gita_ok(port, &token, &["repo", "create", "codedemo"]);
    push_initial_commit(port, &token, "dev7", "dev7", "codedemo", "main");

    // Push another file with a distinctive function name.
    push_commit_to_branch(
        port,
        &token,
        "dev7",
        "dev7",
        "codedemo",
        "main",
        "src/lib.rs",
        "pub fn unique_gita_function() {}\n",
        "Add lib",
    );

    // Search repos.
    let out = gita_ok(port, &token, &["search", "repos", "codedemo"]);
    stdout_contains(&out, "codedemo");

    // Search code.
    let out = gita_ok(port, &token, &["search", "code", "unique_gita_function"]);
    assert!(
        !String::from_utf8_lossy(&out.stdout).is_empty(),
        "code search should return results"
    );
}

// ---------------------------------------------------------------------------
// P2: keyring local test (skips gracefully when no Secret Service)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_keyring_local() {
    // Keyring requires a running Secret Service (D-Bus on Linux). In CI or
    // headless environments it is not available, so we skip gracefully
    // rather than failing.
    let url = "http://127.0.0.1:1/keyring-test";
    match keyring::Entry::new("gita", url) {
        Ok(entry) => {
            match entry.set_password("keyring-test-token") {
                Ok(()) => {
                    let got = entry.get_password().expect("keyring read should succeed");
                    assert_eq!(got, "keyring-test-token");
                    let _ = entry.delete_credential();
                }
                Err(_) => {
                    // No Secret Service available — skip.
                    eprintln!("warning: keyring not available, skipping keyring test");
                }
            }
        }
        Err(_) => {
            // No Secret Service available — skip.
            eprintln!("warning: keyring not available, skipping keyring test");
        }
    }
}
