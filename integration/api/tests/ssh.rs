//! End-to-end SSH integration tests.
//!
//! These tests exercise the real embedded SSH server (`gita-ssh`) with a
//! real `ssh`/`git` client: register a user, add an SSH public key via the
//! REST API, then clone/push repositories over `ssh://`.

mod common;

use common::{create_repo, register_user, request};
use std::path::Path;
use std::process::Command;

/// Generate an ed25519 client key pair into `dir` and return the path to
/// the private key and the public key line.
fn generate_client_key(dir: &Path) -> (std::path::PathBuf, String) {
    let key_path = dir.join("id_ed25519");
    let out = Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-N",
            "",
            "-f",
            key_path.to_str().unwrap(),
            "-q",
        ])
        .output()
        .expect("ssh-keygen must run");
    assert!(out.status.success(), "ssh-keygen failed: {:?}", out);
    let pub_path = dir.join("id_ed25519.pub");
    let pub_line = std::fs::read_to_string(&pub_path).expect("public key file");
    (key_path, pub_line.trim().to_string())
}

/// Start the embedded SSH server for an App and return its port.
async fn start_ssh(app: &gita_server::App) -> u16 {
    let host_key_path = app.data_dir.join("ssh_host_ed25519_key");
    let host_key = gita_ssh::load_or_create_host_key(&host_key_path).unwrap();
    let cfg = gita_ssh::SshServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        host_key,
        repos_root: app.data_dir.join("repos"),
    };
    let handle = gita_ssh::start(cfg, app.db.clone(), app.bus.clone(), app.git.clone())
        .await
        .unwrap();
    handle.addr().port()
}

#[tokio::test(flavor = "multi_thread")]
async fn ssh_clone_and_push_public_repo() {
    let (app, _guard) = common::test_app().await;
    let dir = tempfile::TempDir::new().unwrap();

    // Register user + add SSH key.
    let user = register_user(&app, "alice").await;
    let (key_path, pub_line) = generate_client_key(dir.path());
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/keys",
        Some(serde_json::json!({
            "title": "test-laptop",
            "public_key": pub_line,
        })),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "add key: {body}");

    // Create a public repo.
    create_repo(&app, &user.token, "hello").await;

    let port = start_ssh(&app).await;
    let repo_url = format!("ssh://git@127.0.0.1:{port}/alice/hello.git");

    // Clone over SSH.
    let work = dir.path().join("work");
    let clone = Command::new("git")
        .args(["clone", &repo_url])
        .arg(work.to_str().unwrap())
        .env("GIT_SSH_COMMAND", format!(
            "ssh -p {port} -i {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR",
            key_path.display()
        ))
        .output()
        .expect("git clone must run");
    assert!(
        clone.status.success(),
        "git clone failed: stdout={} stderr={}",
        String::from_utf8_lossy(&clone.stdout),
        String::from_utf8_lossy(&clone.stderr)
    );

    // Commit a file and push back.
    std::fs::write(work.join("README.md"), "# hello\n").unwrap();
    let add = Command::new("git")
        .current_dir(&work)
        .args(["add", "README.md"])
        .output()
        .unwrap();
    assert!(add.status.success());
    let commit = Command::new("git")
        .current_dir(&work)
        .args([
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "add readme",
        ])
        .output()
        .unwrap();
    assert!(commit.status.success(), "commit: {:?}", commit);
    let push = Command::new("git")
        .current_dir(&work)
        .args(["push", "origin", "HEAD:main"])
        .env(
            "GIT_SSH_COMMAND",
            format!(
                "ssh -p {port} -i {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR",
                key_path.display()
            ),
        )
        .output()
        .expect("git push must run");
    assert!(
        push.status.success(),
        "git push failed: stdout={} stderr={}",
        String::from_utf8_lossy(&push.stdout),
        String::from_utf8_lossy(&push.stderr)
    );

    // Verify the push landed by listing commits via the API.
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        "/api/repos/alice/hello/commits",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "commits: {body}");
    let commits = body["commits"].as_array().expect("commits array");
    assert!(
        !commits.is_empty(),
        "expected at least one commit after push"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ssh_private_repo_requires_owner() {
    let (app, _guard) = common::test_app().await;
    let dir = tempfile::TempDir::new().unwrap();

    // Owner alice with SSH key and a private repo.
    let alice = register_user(&app, "alice").await;
    let (alice_key, alice_pub) = generate_client_key(dir.path());
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/keys",
        Some(serde_json::json!({
            "title": "alice-laptop",
            "public_key": alice_pub,
        })),
        Some(&alice.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/repos",
        Some(serde_json::json!({
            "name": "secret",
            "description": "private repo",
            "is_private": true,
            "default_branch": "main",
        })),
        Some(&alice.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create private: {body}");

    // Attacker bob registers his own key (no repo access).
    let bob = register_user(&app, "bob").await;
    let bob_dir = tempfile::TempDir::new().unwrap();
    let (_bob_key, bob_pub) = generate_client_key(bob_dir.path());
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/keys",
        Some(serde_json::json!({
            "title": "bob-laptop",
            "public_key": bob_pub,
        })),
        Some(&bob.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    let port = start_ssh(&app).await;
    let ssh_env = |key: &Path| {
        format!(
            "ssh -p {port} -i {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR",
            key.display()
        )
    };

    // Alice can clone her private repo.
    let alice_work = dir.path().join("alice-work");
    let clone = Command::new("git")
        .args([
            "clone",
            &format!("ssh://git@127.0.0.1:{port}/alice/secret.git"),
        ])
        .arg(alice_work.to_str().unwrap())
        .env("GIT_SSH_COMMAND", ssh_env(&alice_key))
        .output()
        .unwrap();
    assert!(
        clone.status.success(),
        "owner clone failed: {}",
        String::from_utf8_lossy(&clone.stderr)
    );

    // Bob cannot clone the private repo.
    let bob_work = bob_dir.path().join("bob-work");
    let clone = Command::new("git")
        .args([
            "clone",
            &format!("ssh://git@127.0.0.1:{port}/alice/secret.git"),
        ])
        .arg(bob_work.to_str().unwrap())
        .env(
            "GIT_SSH_COMMAND",
            ssh_env(&bob_dir.path().join("id_ed25519")),
        )
        .output()
        .unwrap();
    assert!(
        !clone.status.success(),
        "bob should NOT be able to clone private repo (stderr: {})",
        String::from_utf8_lossy(&clone.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ssh_non_owner_cannot_push() {
    let (app, _guard) = common::test_app().await;
    let dir = tempfile::TempDir::new().unwrap();

    // Owner alice creates a public repo.
    let alice = register_user(&app, "alice").await;
    let (alice_key, alice_pub) = generate_client_key(dir.path());
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/keys",
        Some(serde_json::json!({
            "title": "alice-laptop",
            "public_key": alice_pub,
        })),
        Some(&alice.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    create_repo(&app, &alice.token, "pub").await;

    // Bob has his own key but is not the repo owner.
    let bob = register_user(&app, "bob").await;
    let bob_dir = tempfile::TempDir::new().unwrap();
    let (bob_key, bob_pub) = generate_client_key(bob_dir.path());
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/keys",
        Some(serde_json::json!({
            "title": "bob-laptop",
            "public_key": bob_pub,
        })),
        Some(&bob.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    let port = start_ssh(&app).await;

    // Bob clones the public repo (allowed).
    let bob_work = bob_dir.path().join("bob-work");
    let clone = Command::new("git")
        .args([
            "clone",
            &format!("ssh://git@127.0.0.1:{port}/alice/pub.git"),
        ])
        .arg(bob_work.to_str().unwrap())
        .env(
            "GIT_SSH_COMMAND",
            format!(
                "ssh -p {port} -i {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR",
                bob_key.display()
            ),
        )
        .output()
        .unwrap();
    assert!(
        clone.status.success(),
        "bob clone public failed: {}",
        String::from_utf8_lossy(&clone.stderr)
    );

    // Bob tries to push -> must be rejected.
    std::fs::write(bob_work.join("evil.txt"), "pwned\n").unwrap();
    let add = Command::new("git")
        .current_dir(&bob_work)
        .args(["add", "evil.txt"])
        .output()
        .unwrap();
    assert!(add.status.success());
    let commit = Command::new("git")
        .current_dir(&bob_work)
        .args([
            "-c",
            "user.name=Bob Hacker",
            "-c",
            "user.email=bob@example.com",
            "commit",
            "-m",
            "evil",
        ])
        .output()
        .unwrap();
    assert!(commit.status.success());
    let push = Command::new("git")
        .current_dir(&bob_work)
        .args(["push", "origin", "HEAD:main"])
        .env(
            "GIT_SSH_COMMAND",
            format!(
                "ssh -p {port} -i {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR",
                bob_key.display()
            ),
        )
        .output()
        .unwrap();
    assert!(
        !push.status.success(),
        "bob should NOT be able to push (stderr: {})",
        String::from_utf8_lossy(&push.stderr)
    );
}
/// A group member with the `developer` role may push to the group's
/// repository over SSH.
///
/// The SSH path used to require `user.id == repository.owner_id`, which
/// for a group repository is the id of the user who created the *group*.
/// A developer added to the group could not push, even though the same
/// rule lets them push over HTTP.
#[tokio::test(flavor = "multi_thread")]
async fn ssh_group_developer_can_push_to_a_group_repository() {
    let (app, _guard) = common::test_app().await;
    let dir = tempfile::TempDir::new().unwrap();

    // Alice creates the group and the repository inside it.
    let alice = register_user(&app, "galice").await;
    let (alice_key, alice_pub) = generate_client_key(dir.path());
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/keys",
        Some(serde_json::json!({
            "title": "alice-laptop",
            "public_key": alice_pub,
        })),
        Some(&alice.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups",
        Some(serde_json::json!({"name": "gteam", "description": ""})),
        Some(&alice.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create group: {body}");

    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups/gteam/repos",
        Some(serde_json::json!({
            "name": "shared",
            "description": "",
            "default_branch": "main",
            "is_private": false,
        })),
        Some(&alice.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create repo: {body}");

    // Bob joins the group as a developer and registers his key.
    let bob = register_user(&app, "gbob").await;
    let bob_dir = tempfile::TempDir::new().unwrap();
    let (bob_key, bob_pub) = generate_client_key(bob_dir.path());
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/keys",
        Some(serde_json::json!({
            "title": "bob-laptop",
            "public_key": bob_pub,
        })),
        Some(&bob.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "register key: {body}");

    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/groups/gteam/members",
        Some(serde_json::json!({"username": "gbob", "role": "developer"})),
        Some(&alice.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "add member: {body}");

    let port = start_ssh(&app).await;

    // Bob clones the group's repository.
    let bob_work = bob_dir.path().join("bob-work");
    let clone = Command::new("git")
        .args([
            "clone",
            &format!("ssh://git@127.0.0.1:{port}/gteam/shared.git"),
        ])
        .arg(bob_work.to_str().unwrap())
        .env(
            "GIT_SSH_COMMAND",
            format!(
                "ssh -p {port} -i {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR",
                bob_key.display()
            ),
        )
        .output()
        .unwrap();
    assert!(
        clone.status.success(),
        "developer clone failed: {}",
        String::from_utf8_lossy(&clone.stderr)
    );

    // And pushes to it.
    std::fs::write(bob_work.join("feature.txt"), "from a developer\n").unwrap();
    let add = Command::new("git")
        .current_dir(&bob_work)
        .args(["add", "feature.txt"])
        .output()
        .unwrap();
    assert!(add.status.success());
    let commit = Command::new("git")
        .current_dir(&bob_work)
        .args([
            "-c",
            "user.name=Bob Developer",
            "-c",
            "user.email=bob@example.com",
            "commit",
            "-m",
            "feature",
        ])
        .output()
        .unwrap();
    assert!(commit.status.success());
    let push = Command::new("git")
        .current_dir(&bob_work)
        .args(["push", "origin", "HEAD:main"])
        .env(
            "GIT_SSH_COMMAND",
            format!(
                "ssh -p {port} -i {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR",
                bob_key.display()
            ),
        )
        .output()
        .unwrap();
    assert!(
        push.status.success(),
        "a developer should be able to push to the group's repository (stderr: {})",
        String::from_utf8_lossy(&push.stderr)
    );
}

/// SSH push publishes a push event and triggers CI — the same side
/// effects HTTP push gets, so choosing SSH over HTTP does not change
/// what a repository does.
#[tokio::test(flavor = "multi_thread")]
async fn ssh_push_publishes_events_and_triggers_ci() {
    let (app, _guard) = common::test_app().await;
    let dir = tempfile::TempDir::new().unwrap();

    let user = register_user(&app, "eventalice").await;
    let (key_path, pub_line) = generate_client_key(dir.path());
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/keys",
        Some(serde_json::json!({
            "title": "event-laptop",
            "public_key": pub_line,
        })),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "add key: {body}");

    let _ = common::create_repo_with(
        &app,
        &user.token,
        "eventrepo",
        Some(serde_json::json!({"is_private": false})),
    )
    .await;

    let port = start_ssh(&app).await;
    let repo_url = format!("ssh://git@127.0.0.1:{port}/eventalice/eventrepo.git");

    let work = dir.path().join("work");
    let clone = Command::new("git")
        .args(["clone", &repo_url])
        .arg(work.to_str().unwrap())
        .env("GIT_SSH_COMMAND", format!(
            "ssh -p {port} -i {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR",
            key_path.display()
        ))
        .output()
        .expect("git clone must run");
    assert!(
        clone.status.success(),
        "clone failed: {}",
        String::from_utf8_lossy(&clone.stderr)
    );

    // Commit a CI configuration along with a file.
    std::fs::write(
        work.join(".gita-ci.yml"),
        "stages: [build]\njobs:\n  build:\n    stage: build\n    script: [echo hello]\n",
    )
    .unwrap();
    std::fs::write(work.join("README.md"), "# eventrepo\n").unwrap();
    let add = Command::new("git")
        .current_dir(&work)
        .args(["add", "."])
        .output()
        .unwrap();
    assert!(add.status.success());
    let commit = Command::new("git")
        .current_dir(&work)
        .args([
            "-c",
            "user.name=Event Alice",
            "-c",
            "user.email=alice@example.com",
            "commit",
            "-m",
            "add ci config",
        ])
        .output()
        .unwrap();
    assert!(commit.status.success());
    let push = Command::new("git")
        .current_dir(&work)
        .args(["push", "origin", "HEAD:main"])
        .env("GIT_SSH_COMMAND", format!(
            "ssh -p {port} -i {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR",
            key_path.display()
        ))
        .output()
        .expect("git push must run");
    assert!(
        push.status.success(),
        "push failed: {}",
        String::from_utf8_lossy(&push.stderr)
    );

    // The push event is published. The push's side effects happen in a
    // background task, so give them a moment rather than racing them.
    let mut events: Vec<serde_json::Value> = Vec::new();
    for _ in 0..40 {
        let (status, body) = request(
            &app,
            axum::http::Method::GET,
            "/api/events",
            None,
            Some(&user.token),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK, "events: {body}");
        events = body["events"].as_array().cloned().unwrap_or_default();
        if events.iter().any(|e| e["event_type"] == "push") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        events.iter().any(|e| e["event_type"] == "push"),
        "expected a push event to be published, got {events:?}"
    );

    // The CI run was enqueued. The push's side effects happen in a
    // background task, so give them a moment rather than racing them.
    let mut runs: Vec<serde_json::Value> = Vec::new();
    for _ in 0..40 {
        let (status, body) = request(
            &app,
            axum::http::Method::GET,
            "/api/repos/eventalice/eventrepo/pipelines/.gita-ci.yml/runs",
            None,
            Some(&user.token),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK, "runs: {body}");
        runs = body["runs"].as_array().cloned().unwrap_or_default();
        if !runs.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        !runs.is_empty(),
        "expected at least one CI run to be enqueued"
    );
    assert!(
        runs.iter()
            .any(|r| r["status"] == "queued" || r["status"] == "running"),
        "expected a running or queued run, got {runs:?}"
    );
}

/// A rejected push publishes nothing and triggers no CI, over SSH too.
#[tokio::test(flavor = "multi_thread")]
async fn ssh_rejected_push_publishes_nothing() {
    let (app, _guard) = common::test_app().await;
    let dir = tempfile::TempDir::new().unwrap();

    let owner = register_user(&app, "owner-alice").await;
    let intruder = register_user(&app, "intruder-bob").await;
    let (bob_key, pub_line) = generate_client_key(dir.path());
    let (status, _) = request(
        &app,
        axum::http::Method::POST,
        "/api/auth/keys",
        Some(serde_json::json!({
            "title": "intruder-key",
            "public_key": pub_line,
        })),
        Some(&intruder.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // A public repository of the owner — the intruder may read it but
    // not push to it.
    let _ = common::create_repo(&app, &owner.token, "public").await;

    let port = start_ssh(&app).await;
    let ssh_env = format!(
        "ssh -p {port} -i {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR",
        bob_key.display()
    );

    let work = dir.path().join("intruder-work");
    let clone = Command::new("git")
        .args([
            "clone",
            &format!("ssh://git@127.0.0.1:{port}/owner-alice/public.git"),
        ])
        .arg(work.to_str().unwrap())
        .env("GIT_SSH_COMMAND", &ssh_env)
        .output()
        .unwrap();
    assert!(
        clone.status.success(),
        "clone failed: {}",
        String::from_utf8_lossy(&clone.stderr)
    );

    std::fs::write(work.join("bad.txt"), "nope\n").unwrap();
    let add = Command::new("git")
        .current_dir(&work)
        .args(["add", "bad.txt"])
        .output()
        .unwrap();
    assert!(add.status.success());
    let commit = Command::new("git")
        .current_dir(&work)
        .args([
            "-c",
            "user.name=Intruder",
            "-c",
            "user.email=i@example.com",
            "commit",
            "-m",
            "bad",
        ])
        .output()
        .unwrap();
    assert!(commit.status.success());
    let push = Command::new("git")
        .current_dir(&work)
        .args(["push", "origin", "HEAD:main"])
        .env("GIT_SSH_COMMAND", &ssh_env)
        .output()
        .unwrap();
    assert!(
        !push.status.success(),
        "intruder push must be rejected: {}",
        String::from_utf8_lossy(&push.stderr)
    );

    // No push event should have been published. The side effects of a
    // push happen in a background task, so wait long enough for them to
    // have happened — otherwise this would pass for the wrong reason.
    let mut events: Vec<serde_json::Value> = Vec::new();
    for _ in 0..10 {
        let (status, body) = request(
            &app,
            axum::http::Method::GET,
            "/api/events",
            None,
            Some(&owner.token),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK, "events: {body}");
        events = body["events"].as_array().cloned().unwrap_or_default();
        if events.iter().any(|e| e["event_type"] == "push") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        events.iter().all(|e| e["event_type"] != "push"),
        "a rejected push must not publish an event, got {events:?}"
    );
}
