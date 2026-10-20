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
    let handle = gita_ssh::start(cfg, app.db.clone()).await.unwrap();
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
        .args(["commit", "-m", "add readme"])
        .output()
        .unwrap();
    assert!(commit.status.success(), "commit: {:?}", commit);
    let push = Command::new("git")
        .current_dir(&work)
        .args(["push", "origin", "HEAD"])
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
    assert!(!commits.is_empty(), "expected at least one commit after push");
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
        .env("GIT_SSH_COMMAND", ssh_env(&bob_dir.path().join("id_ed25519")))
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
        .args(["commit", "-m", "evil"])
        .output()
        .unwrap();
    assert!(commit.status.success());
    let push = Command::new("git")
        .current_dir(&bob_work)
        .args(["push", "origin", "HEAD"])
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