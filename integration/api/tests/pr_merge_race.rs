//! Concurrent merges of one pull request.
//!
//! Merging used to be a chain of non-atomic steps: read the PR, check
//! `status != Open`, run git, then write the new status. Two requests that
//! arrived together both passed the check, both ran `git merge`, and the
//! target branch ended up with **two merge commits for one PR** — the
//! second merge saw the first one's commit as its target, so the PR's
//! changes were folded in twice.
//!
//! The fix makes the status change the arbiter: `open → merged` is a
//! single conditional UPDATE, so exactly one request wins the right to run
//! git and the loser gets a 409 without touching the repository.
//!
//! These tests are what pins that down. They deliberately build a
//! non-fast-forward merge: with a fast-forward there is no merge commit to
//! count, and the duplicate would be invisible.

mod common;

use common::{register_user, request};
use std::process::Command;

/// Fixture: a repo whose `main` and `feature` have diverged, and an open PR
/// from `feature` into `main`. Returns `(owner, name, pr_number)`.
async fn diverged_repo_with_open_pr(app: &gita_admin::App, token: &str) -> (String, String, i64) {
    let full = common::create_repo(app, token, "racerepo").await;
    let (owner, name) = full.split_once('/').unwrap().to_string_pair();
    common::push_initial_commit(app, &owner, &name);

    let bare = app
        .data_dir
        .join("repos")
        .join(&owner)
        .join(format!("{name}.git"));
    let bare_url = format!("file://{}", bare.display());

    fn git(work: &std::path::Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(work)
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@e.io")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@e.io")
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let work_parent = app.data_dir.join("work-pr-race");
    let work = work_parent.join("wt");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();

    git(&work, &["clone", "-q", "-b", "main", &bare_url, "."]);
    std::fs::write(work.join("feature.txt"), "feature\n").unwrap();
    git(&work, &["checkout", "-q", "-b", "feature"]);
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "feature work"]);
    git(&work, &["push", "-q", "-u", "origin", "feature"]);

    git(&work, &["checkout", "-q", "main"]);
    std::fs::write(work.join("main.txt"), "main\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "main work"]);
    git(&work, &["push", "-q", "origin", "main"]);

    let uri_prs = format!("/api/repos/{owner}/{name}/prs");
    let (status, body) = request(
        app,
        axum::http::Method::POST,
        &uri_prs,
        Some(serde_json::json!({
            "title": "race",
            "body": "",
            "source_branch": "feature",
            "target_branch": "main",
        })),
        Some(token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create pr: {body}");

    let _ = std::fs::remove_dir_all(&work_parent);
    (owner, name, body["number"].as_i64().unwrap())
}

/// Small helper so a `"a/b"` can be split into two owned strings without
/// tripping the borrow checker over the temporary.
trait StringPair {
    fn to_string_pair(self) -> (String, String);
}

impl StringPair for (&str, &str) {
    fn to_string_pair(self) -> (String, String) {
        (self.0.to_string(), self.1.to_string())
    }
}

/// Count the commits on `main` that have more than one parent — i.e. the
/// merge commits. This is the measurement the bug violates.
fn merge_commit_count(app: &gita_admin::App, owner: &str, name: &str) -> usize {
    let bare = app
        .data_dir
        .join("repos")
        .join(owner)
        .join(format!("{name}.git"));
    let out = Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().unwrap(),
            "rev-list",
            "--min-parents=2",
            "main",
        ])
        .output()
        .expect("git rev-list");
    assert!(
        out.status.success(),
        "rev-list failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

/// Two merges of the same PR, started together, must produce exactly one
/// merge commit and exactly one "merged" answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_concurrent_merges_produce_one_merge_commit() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "race-merge").await;
    let (owner, name, number) = diverged_repo_with_open_pr(&app, &user.token).await;

    let app_a = app.clone();
    let app_b = app.clone();
    let token_a = user.token.clone();
    let token_b = user.token.clone();
    let uri = format!("/api/repos/{owner}/{name}/prs/{number}/merge");

    let merge = |app: gita_admin::App, token: String, uri: String| async move {
        request(
            &app,
            axum::http::Method::POST,
            &uri,
            None,
            Some(&token),
        )
        .await
    };

    // Start both before awaiting either: the status claim is what decides.
    let (first, second) = tokio::join!(merge(app_a, token_a, uri.clone()), merge(app_b, token_b, uri));

    let statuses = [first.0, second.0];
    let ok = statuses
        .iter()
        .filter(|s| **s == axum::http::StatusCode::OK)
        .count();
    let conflict = statuses
        .iter()
        .filter(|s| **s == axum::http::StatusCode::CONFLICT)
        .count();

    assert_eq!(
        ok, 1,
        "exactly one merge may succeed, got {statuses:?} (bodies: {first:?} {second:?})"
    );
    assert_eq!(
        conflict, 1,
        "the losing merge must be a 409, got {statuses:?}"
    );

    assert_eq!(
        merge_commit_count(&app, &owner, &name),
        1,
        "the target branch must carry exactly one merge commit for one PR"
    );

    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/prs/{number}"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["pr"]["status"], "merged");
}

/// A merge that fails on a conflict must not leave the PR stuck "merged":
/// the claim is reverted so the PR can be fixed and retried.
#[tokio::test]
async fn a_failed_merge_reverts_the_claim() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "conflict-merge").await;
    let full = common::create_repo(&app, &user.token, "conflict-repo").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);

    let bare = app
        .data_dir
        .join("repos")
        .join(owner)
        .join(format!("{name}.git"));
    let bare_url = format!("file://{}", bare.display());

    fn git(work: &std::path::Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(work)
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@e.io")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@e.io")
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let work_parent = app.data_dir.join("work-conflict");
    let work = work_parent.join("wt");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();

    // Both branches edit the same file differently → a real conflict.
    git(&work, &["clone", "-q", "-b", "main", &bare_url, "."]);
    std::fs::write(work.join("shared.txt"), "from feature\n").unwrap();
    git(&work, &["checkout", "-q", "-b", "feature"]);
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "feature"]);
    git(&work, &["push", "-q", "-u", "origin", "feature"]);

    git(&work, &["checkout", "-q", "main"]);
    std::fs::write(work.join("shared.txt"), "from main\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "main"]);
    git(&work, &["push", "-q", "origin", "main"]);

    let uri_prs = format!("/api/repos/{owner}/{name}/prs");
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &uri_prs,
        Some(serde_json::json!({
            "title": "conflicting",
            "body": "",
            "source_branch": "feature",
            "target_branch": "main",
        })),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create pr: {body}");
    let number = body["number"].as_i64().unwrap();

    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &format!("/api/repos/{owner}/{name}/prs/{number}/merge"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::CONFLICT,
        "a conflicting merge must be a 409, got {body}"
    );

    // The claim was reverted: the PR is still open, not stranded "merged".
    let (status, body) = request(
        &app,
        axum::http::Method::GET,
        &format!("/api/repos/{owner}/{name}/prs/{number}"),
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(
        body["pr"]["status"], "open",
        "a failed merge must leave the PR open for a retry, got {body}"
    );

    let _ = std::fs::remove_dir_all(&work_parent);
}

/// Closing is the same kind of race: two concurrent closes, or a close
/// racing a merge, must not both report success.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_concurrent_closes_only_one_wins() {
    let (app, _guard) = common::test_app().await;
    let user = register_user(&app, "race-close").await;
    let full = common::create_repo(&app, &user.token, "close-repo").await;
    let (owner, name) = full.split_once('/').unwrap();
    common::push_initial_commit(&app, owner, name);

    let uri_prs = format!("/api/repos/{owner}/{name}/prs");
    let (status, body) = request(
        &app,
        axum::http::Method::POST,
        &uri_prs,
        Some(serde_json::json!({
            "title": "close race",
            "body": "",
            "source_branch": "main",
            "target_branch": "main",
        })),
        Some(&user.token),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "create pr: {body}");
    let number = body["number"].as_i64().unwrap();

    let uri = format!("/api/repos/{owner}/{name}/prs/{number}/close");
    let close = |app: gita_admin::App, token: String, uri: String| async move {
        request(&app, axum::http::Method::POST, &uri, None, Some(&token)).await
    };
    let (a, b) = tokio::join!(
        close(app.clone(), user.token.clone(), uri.clone()),
        close(app.clone(), user.token.clone(), uri)
    );

    let ok = [a.0, b.0]
        .iter()
        .filter(|s| **s == axum::http::StatusCode::OK)
        .count();
    assert_eq!(ok, 1, "exactly one close may win, got {:?} and {:?}", a.0, b.0);
}
