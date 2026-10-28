mod common;

use common::*;

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

    let origin = format!("http://dev5:{token}@127.0.0.1:{port}/dev5/pipedemo.git");
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

#[tokio::test(flavor = "multi_thread")]
async fn e2e_ssh_key() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "sshuser");

    // A syntactically valid OpenSSH public key line (server validates the
    // algorithm prefix and base64 blob, not whether the key is real).
    let pubkey =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIB32vUpOqxd60GVVNxMePF3cdub6Q0yRIXQlVxP4vTa7 gita-e2e";

    // Add the key.
    let out = gita_ok(
        port,
        &token,
        &["ssh-key", "add", "--title", "e2e-laptop", pubkey],
    );
    stdout_contains(&out, "e2e-laptop");

    // List keys — should include the new one.
    let out = gita_ok(port, &token, &["ssh-key", "list"]);
    stdout_contains(&out, "e2e-laptop");

    // Get the key id from JSON.
    let keys: serde_json::Value = gita_json(port, &token, &["ssh-key", "list"]);
    let empty = serde_json::Value::Array(vec![]);
    let key_list = keys["keys"]
        .as_array()
        .unwrap_or(&empty.as_array().unwrap());
    let key_id = key_list
        .iter()
        .find(|k| k.get("title").and_then(|t| t.as_str()) == Some("e2e-laptop"))
        .expect("e2e-laptop key in list")
        .get("id")
        .and_then(|i| i.as_i64())
        .expect("key id");

    // Delete the key.
    let out = gita_ok(
        port,
        &token,
        &["ssh-key", "delete", "--id", &key_id.to_string()],
    );
    stdout_contains(&out, "deleted");

    // Listing again should not include it.
    let out = gita_ok(port, &token, &["ssh-key", "list"]);
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("e2e-laptop"),
        "e2e-laptop key should be removed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_group_members() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let owner_token = register_via_binary(port, "grpowner");
    let member_token = register_via_binary(port, "grpmember");

    // Owner creates a group.
    gita_ok(
        port,
        &owner_token,
        &["group", "create", "eng", "--description", "Eng group"],
    );

    // Add the member.
    let out = gita_ok(
        port,
        &owner_token,
        &[
            "group",
            "member-add",
            "eng",
            "grpmember",
            "--role",
            "maintainer",
        ],
    );
    stdout_contains(&out, "grpmember");

    // List members — should include them.
    let out = gita_ok(port, &owner_token, &["group", "members", "eng"]);
    stdout_contains(&out, "grpmember");

    // Set role.
    let out = gita_ok(
        port,
        &owner_token,
        &[
            "group",
            "member-set-role",
            "eng",
            "grpmember",
            "--role",
            "developer",
        ],
    );
    stdout_contains(&out, "developer");

    // Remove the member.
    let out = gita_ok(
        port,
        &owner_token,
        &["group", "member-remove", "eng", "grpmember"],
    );
    stdout_contains(&out, "member removed");

    // Members list should no longer contain them.
    let out = gita_ok(port, &owner_token, &["group", "members", "eng"]);
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("grpmember"),
        "grpmember should be removed from group"
    );

    // member_token can still authenticate (unused here).
    let _ = member_token;
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_event_filter() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "evuser");

    // Create a repo and push, which emits a push event.
    gita_ok(port, &token, &["repo", "create", "evdemo"]);
    push_initial_commit(port, &token, "evuser", "evuser", "evdemo", "main");

    // Poll until a push event for this repo shows up.
    let mut found = false;
    for _ in 0..30 {
        let out = gita_ok(
            port,
            &token,
            &["event", "list", "--repo", "evuser/evdemo", "--limit", "50"],
        );
        if String::from_utf8_lossy(&out.stdout).contains("push") {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(found, "expected a push event for evuser/evdemo");

    // Type filter also works.
    let out = gita_ok(
        port,
        &token,
        &["event", "list", "--type", "push", "--limit", "50"],
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("push"),
        "event list --type push should contain push events"
    );
}

#[tokio::test]
async fn e2e_keyring_local() {
    // Keyring requires a running Secret Service (D-Bus on Linux). In CI or
    // headless environments it is not available, so we skip gracefully
    // rather than failing.
    //
    // It can also *block* instead of failing: a locked Secret Service waits
    // for an unlock agent that will never come, and the kernel keyring
    // backend waits in `request_key()` for a user keyring. The probe
    // therefore runs on a worker thread with a deadline.
    let url = "http://127.0.0.1:1/keyring-test";
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = keyring::Entry::new("gita", url).and_then(|entry| {
            entry.set_password("keyring-test-token")?;
            let got = entry.get_password()?;
            let _ = entry.delete_credential();
            Ok(got)
        });
        let _ = tx.send(result);
    });
    match rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(Ok(got)) => assert_eq!(got, "keyring-test-token"),
        Ok(Err(e)) => {
            eprintln!("warning: keyring not available, skipping keyring test: {e}");
        }
        Err(_) => {
            eprintln!("warning: keyring call did not return, skipping keyring test");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_admin_user_management() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    // The first account, created through init, is the administrator.
    let admin_token = init_admin_via_http(port, "rootadmin").await;

    // A normal self-registered account.
    let staff_token = register_via_binary(port, "staff");

    // A non-admin is rejected before anything else happens.
    let out = gita(port, &staff_token, &["admin", "list"]);
    gita_fails(&out);
    stderr_contains(&out, "403");

    // The admin sees every account in the list.
    let out = gita_ok(port, &admin_token, &["admin", "list"]);
    stdout_contains(&out, "rootadmin");
    stdout_contains(&out, "staff");

    // The admin creates an account for someone else.
    let out = gita_ok(
        port,
        &admin_token,
        &[
            "admin",
            "create",
            "newbie",
            "--email",
            "newbie@e2e.local",
            "--password",
            "hunter22",
        ],
    );
    stdout_contains(&out, "newbie");

    // Duplicate creation is a conflict.
    let out = gita(
        port,
        &admin_token,
        &[
            "admin",
            "create",
            "newbie",
            "--email",
            "other@e2e.local",
            "--password",
            "hunter22",
        ],
    );
    gita_fails(&out);
    stderr_contains(&out, "409");

    // Grant, then revoke the admin role.
    let out = gita_ok(port, &admin_token, &["admin", "set-admin", "newbie"]);
    stdout_contains(&out, "admin: true");
    let out = gita_ok(
        port,
        &admin_token,
        &["admin", "set-admin", "newbie", "--admin=false"],
    );
    stdout_contains(&out, "admin: false");

    // The very last admin cannot be demoted — otherwise the system would
    // be left without anyone able to manage it.
    let out = gita(
        port,
        &admin_token,
        &["admin", "set-admin", "rootadmin", "--admin=false"],
    );
    gita_fails(&out);
    stderr_contains(&out, "400");

    // An admin cannot delete their own account.
    let out = gita(port, &admin_token, &["admin", "delete", "rootadmin"]);
    gita_fails(&out);
    stderr_contains(&out, "400");

    // Reset a password without knowing the old one.
    let out = gita_ok(
        port,
        &admin_token,
        &[
            "admin",
            "reset-password",
            "newbie",
            "--password",
            "fresh-pass-1",
        ],
    );
    stdout_contains(&out, "Password reset for newbie");

    // Delete the account; it disappears from the list.
    let out = gita_ok(port, &admin_token, &["admin", "delete", "newbie"]);
    stdout_contains(&out, "deleted");
    let out = gita_ok(port, &admin_token, &["admin", "list"]);
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("newbie"),
        "deleted user should not appear in admin list"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_api_call() {
    let server = Server::new().await.expect("boot server");
    let port = server.port();

    let token = register_via_binary(port, "apiuser");

    // Create a repo so the list is non-empty.
    gita_ok(port, &token, &["repo", "create", "apidemo"]);

    // Raw GET with /api prefix.
    let out = gita_ok(port, &token, &["api", "/api/repos"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("apidemo"),
        "api /api/repos should list apidemo"
    );

    // Raw GET without /api prefix is normalised.
    let out = gita_ok(port, &token, &["api", "repos"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("apidemo"),
        "api repos should list apidemo"
    );

    // Raw POST with a JSON body (create a second repo).
    let out = gita_ok(
        port,
        &token,
        &[
            "api",
            "repos",
            "--method",
            "POST",
            "--body",
            r#"{"name":"apidemo2"}"#,
        ],
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("apidemo2"),
        "api POST repos should create apidemo2"
    );
}
