//! SSH fails closed when the database does.
//!
//! The SSH transport asks the database who may touch a repository. When
//! that question cannot be answered — the query errors — the answer
//! must be "no": a transport that opened the repository on a failed
//! lookup would hand out source code on the strength of an error.

mod common;

use std::path::Path;
use std::process::Command;

/// Generate an ed25519 client key pair into `dir` and return the public
/// key line.
fn generate_client_key(dir: &Path) -> (std::path::PathBuf, String) {
    let key_path = dir.join("id_ed25519");
    let out = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-f", key_path.to_str().unwrap(), "-q"])
        .output()
        .expect("ssh-keygen must run");
    assert!(out.status.success(), "ssh-keygen failed: {:?}", out);
    let pub_line = std::fs::read_to_string(dir.join("id_ed25519.pub")).expect("public key file");
    (key_path, pub_line.trim().to_string())
}

/// Start the embedded SSH server for an App and return its port.
async fn start_ssh(app: &gita_admin::App) -> u16 {
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

/// A group member's clone of a group repository is answered, not
/// served, when the database cannot say whether they may read it.
///
/// The repository belongs to a group, so the access check has to ask
/// for the caller's group role; dropping the table that holds the
/// roles makes that question fail. The handler must refuse — with the
/// refusal of a private repository, not with the source code.
#[tokio::test(flavor = "multi_thread")]
async fn ssh_fails_closed_when_the_database_errors() {
    let (app, _guard) = common::test_app().await;
    let dir = tempfile::TempDir::new().unwrap();

    let user = common::register_user(&app, "fcdrop").await;
    let (key_path, pub_line) = generate_client_key(dir.path());
    let (status, body) = common::request(
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

    // A group repository the user can read.
    common::request(
        &app,
        axum::http::Method::POST,
        "/api/groups",
        Some(serde_json::json!({"name": "fcgroup", "description": ""})),
        Some(&user.token),
    )
    .await;
    let (status, body) = common::request(
        &app,
        axum::http::Method::POST,
        "/api/groups/fcgroup/repos",
        Some(serde_json::json!({
            "name": "shared",
            "description": "",
            "default_branch": "main",
            "is_private": true,
        })),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create repo: {body}");

    let port = start_ssh(&app).await;
    let repo_url = format!("ssh://git@127.0.0.1:{port}/fcgroup/shared.git");

    // A work tree to clone into.
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    let clone = |url: &str, into: &Path| {
        Command::new("git")
            .args(["clone", url, "."])
            .current_dir(into)
            .env(
                "GIT_SSH_COMMAND",
                format!(
                    "ssh -i {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null",
                    key_path.display()
                ),
            )
            .output()
            .expect("git clone must run")
    };

    // Sanity check: while the database is whole, the clone works.
    let out = clone(&repo_url, &work);
    assert!(
        out.status.success(),
        "the clone must work before the database breaks:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // Break the question the access check asks: the group roles are
    // gone, so the check cannot be answered for a group repository.
    sqlx::query("DROP TABLE IF EXISTS group_members CASCADE")
        .execute(app.db.pool())
        .await
        .unwrap();

    let work2 = dir.path().join("work2");
    std::fs::create_dir_all(&work2).unwrap();
    let out = clone(&repo_url, &work2);
    assert!(
        !out.status.success(),
        "a clone must not succeed when the access check cannot be answered"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("fn main"),
        "no source code may be served: {stderr}"
    );
}
