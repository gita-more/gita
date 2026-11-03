mod common;

use assert_cmd::Command as AssertCmd;
use common::*;

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

#[tokio::test(flavor = "multi_thread")]
async fn e2e_auth_me_update() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "meuser");

    // Update locale.
    let out = gita_ok(port, &token, &["auth", "me", "--locale", "zh"]);
    stdout_contains(&out, "zh");

    // whoami should reflect the change.
    let out = gita_ok(port, &token, &["auth", "whoami"]);
    stdout_contains(&out, "zh");

    // No-op update should fail gracefully (non-zero exit) but not crash.
    let out = gita(port, &token, &["auth", "me"]);
    assert!(
        !out.status.success(),
        "auth me with nothing to update should fail"
    );
}

/// The git credential helper must hand git the stored username and token.
///
/// This is what makes `git push` work without pasting a token into git's
/// prompt: git asks the helper for the credentials of the host it is
/// talking to, and the helper answers from the credential store.
#[test]
fn e2e_credential_helper_serves_stored_credentials() {
    // This test owns its credential file. The shared test config dir is
    // written by every other `gita` invocation in this binary; running in
    // parallel with them truncated the file before the helper read it.
    let config = tempfile::TempDir::new().expect("create config dir");
    let dir = config.path().to_path_buf();
    std::fs::write(
        dir.join("credentials.toml"),
        "[sites.\"http://127.0.0.1:3100\"]\ntoken = \"helper-token\"\nusername = \"helper-user\"\n",
    )
    .expect("write credentials");

    let mut cmd = AssertCmd::new(gita_bin());
    cmd.env("GITA_DISABLE_KEYRING", "1")
        .env("GITA_CONFIG_DIR", &dir)
        .args(["auth", "credential", "get"])
        .write_stdin("protocol=http\nhost=127.0.0.1:3100\npath=owner/repo.git\n\n");
    let out = cmd.output().expect("spawn gita");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("username=helper-user"),
        "expected the stored username, got: {stdout}"
    );
    assert!(
        stdout.contains("password=helper-token"),
        "expected the stored token, got: {stdout}"
    );

    // An unknown host must yield no credential — git then falls back to
    // its own prompt.
    let mut cmd = AssertCmd::new(gita_bin());
    cmd.env("GITA_DISABLE_KEYRING", "1")
        .env("GITA_CONFIG_DIR", &dir)
        .args(["auth", "credential", "get"])
        .write_stdin("protocol=https\nhost=unknown.example.com\n\n");
    let out = cmd.output().expect("spawn gita");
    assert!(
        out.status.success(),
        "an unknown host must not fail the helper"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "an unknown host must yield no credential"
    );
}
