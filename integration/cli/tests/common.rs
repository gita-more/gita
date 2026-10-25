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
pub use tempfile::TempDir;
use tokio::net::TcpListener;

/// Database URL for integration tests. Override via `GITA_TEST_DATABASE_URL`.
pub fn test_db_url() -> String {
    std::env::var("GITA_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://gita:gita@127.0.0.1:55432/gita".to_string())
}

/// Path to the `gita` CLI binary.
pub fn gita_bin() -> PathBuf {
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
pub fn gita(port: u16, token: &str, args: &[&str]) -> std::process::Output {
    let mut cmd = AssertCmd::new(gita_bin());
    cmd.env("GITA_URL", format!("http://127.0.0.1:{port}"))
        .env("GITA_TOKEN", token);
    for a in args {
        cmd.arg(a);
    }
    cmd.output().expect("spawn gita")
}

/// Assert that a `gita` invocation succeeds.
pub fn gita_ok(port: u16, token: &str, args: &[&str]) -> std::process::Output {
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
pub fn gita_json(port: u16, token: &str, args: &[&str]) -> serde_json::Value {
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
pub fn stdout_contains(out: &std::process::Output, needle: &str) {
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(needle),
        "expected stdout to contain {:?}, got:\n{}",
        needle,
        String::from_utf8_lossy(&out.stdout)
    );
}

/// Stderr contains the given substring.
pub fn stderr_contains(out: &std::process::Output, needle: &str) {
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(needle),
        "expected stderr to contain {:?}, got:\n{}",
        needle,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Assert that a `gita` invocation fails (non-zero exit).
pub fn gita_fails(out: &std::process::Output) {
    assert!(
        !out.status.success(),
        "expected gita invocation to fail, but it succeeded:\nstdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A running server bound to an ephemeral port.
pub struct Server {
    addr: SocketAddr,
    _guard: TestAppGuard,
}

impl Server {
    pub async fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
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

    pub fn port(&self) -> u16 {
        self.addr.port()
    }
}

/// Boot a real `gita_server::App` with an isolated Postgres schema.
pub async fn test_app(
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
pub struct TestAppGuard {
    _base_url: String,
    _schema: String,
    _tmp: TempDir,
}

pub fn register_via_binary(port: u16, username: &str) -> String {
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
pub fn push_initial_commit(
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
pub fn push_commit_to_branch(
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

/// Create the first administrator via POST /api/auth/init and return its
/// JWT. The e2e database schema starts empty, so init always succeeds —
/// and the account it creates is the system administrator.
pub async fn init_admin_via_http(port: u16, username: &str) -> String {
    let client = reqwest::Client::new();
    let res = client
        .post(format!("http://127.0.0.1:{port}/api/auth/init"))
        .json(&serde_json::json!({
            "username": username,
            "email": format!("{username}@e2e.local"),
            "password": "hunter22",
        }))
        .send()
        .await
        .expect("init request");
    assert!(
        res.status().is_success(),
        "auth/init failed: {}",
        res.status()
    );
    let v: serde_json::Value = res.json().await.unwrap();
    v["token"].as_str().unwrap().to_string()
}
