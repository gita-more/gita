//! The `/dir` endpoint — the non-recursive directory listing the
//! GitHub-style code page uses.
//!
//! `/tree` answers the same question recursively and has been covered
//! for a while; `/dir` was added later and had no test at all, even
//! though the code page renders from it.

mod common;

use axum::http::{Method, StatusCode};
use common::{create_repo, create_repo_with, push_files, register_user, request, test_app};

/// The entry names of a `/dir` response.
fn entry_names(body: &serde_json::Value) -> Vec<String> {
    body["entries"]
        .as_array()
        .expect("entries is an array")
        .iter()
        .map(|e| e["name"].as_str().expect("entry name").to_string())
        .collect()
}

/// The root listing holds the top-level entries only.
#[tokio::test]
async fn dir_lists_the_root_without_descending() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "diruser").await;
    create_repo(&app, &user.token, "listing").await;
    push_files(
        &app,
        "diruser",
        "listing",
        &[
            ("README.md", b"# hello\n"),
            ("src/main.rs", b"fn main() {}\n"),
            ("docs/guide.md", b"guide\n"),
        ],
        "main",
        "initial commit",
    );

    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/diruser/listing/dir",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "dir listing: {body}");

    let mut names = entry_names(&body);
    names.sort();
    assert_eq!(
        names,
        vec!["README.md", "docs", "src"],
        "root listing: {body}"
    );

    // The listing is not recursive: the file inside `src` is not listed.
    assert!(
        !names.iter().any(|n| n.contains('/')),
        "the root listing must not descend: {body}"
    );

    // Directories are marked as trees, files as blobs.
    let entries = body["entries"].as_array().unwrap();
    let src = entries
        .iter()
        .find(|e| e["name"] == "src")
        .expect("src entry");
    assert_eq!(src["kind"], "tree", "src should be a tree: {body}");
    let readme = entries
        .iter()
        .find(|e| e["name"] == "README.md")
        .expect("README entry");
    assert_eq!(readme["kind"], "blob", "README.md should be a blob: {body}");

    // The response echoes the path and the ref it answered for.
    assert_eq!(body["path"], "", "path: {body}");
    assert_eq!(body["ref"], "main", "ref: {body}");
}

/// A subdirectory is listed by its path.
#[tokio::test]
async fn dir_lists_a_subdirectory() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "dirsub").await;
    create_repo(&app, &user.token, "nested").await;
    push_files(
        &app,
        "dirsub",
        "nested",
        &[
            ("src/main.rs", b"fn main() {}\n"),
            ("src/lib.rs", b"pub fn f() {}\n"),
            ("README.md", b"# hello\n"),
        ],
        "main",
        "initial commit",
    );

    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/dirsub/nested/dir?path=src",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "dir listing: {body}");

    let mut names = entry_names(&body);
    names.sort();
    assert_eq!(names, vec!["lib.rs", "main.rs"], "subdirectory: {body}");
    assert_eq!(body["path"], "src", "path: {body}");
}

/// The listing follows the requested ref.
#[tokio::test]
async fn dir_honours_the_ref_query() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "dirref").await;
    create_repo(&app, &user.token, "branchy").await;
    push_files(
        &app,
        "dirref",
        "branchy",
        &[("main.txt", b"main\n")],
        "main",
        "initial commit",
    );
    push_files(
        &app,
        "dirref",
        "branchy",
        &[("feature.txt", b"feature\n")],
        "feature",
        "feature commit",
    );

    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/dirref/branchy/dir?ref=feature",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "dir listing: {body}");
    // The feature branch was committed on top of `main`, so it carries
    // both files.
    let mut names = entry_names(&body);
    names.sort();
    assert_eq!(
        names,
        vec!["feature.txt".to_string(), "main.txt".to_string()],
        "listing of the feature branch: {body}"
    );
    assert_eq!(body["ref"], "feature", "ref: {body}");

    // The default branch still shows its own file.
    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/dirref/branchy/dir",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "dir listing: {body}");
    assert_eq!(
        entry_names(&body),
        vec!["main.txt".to_string()],
        "listing of the default branch: {body}"
    );
}

/// A ref that does not exist is an error, not an empty listing.
#[tokio::test]
async fn dir_of_a_missing_ref_is_404() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "dirbad").await;
    create_repo(&app, &user.token, "there").await;
    push_files(
        &app,
        "dirbad",
        "there",
        &[("a.txt", b"a\n")],
        "main",
        "initial commit",
    );

    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/dirbad/there/dir?ref=nope",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "dir listing: {body}");
}

/// A private repository is not listed for an anonymous caller.
#[tokio::test]
async fn dir_of_a_private_repo_is_404_for_anonymous_callers() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "dirprivate").await;
    create_repo_with(
        &app,
        &user.token,
        "secret",
        Some(serde_json::json!({"is_private": true})),
    )
    .await;
    push_files(
        &app,
        "dirprivate",
        "secret",
        &[("secret.txt", b"secret\n")],
        "main",
        "initial commit",
    );

    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/dirprivate/secret/dir",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "dir listing: {body}");
    assert!(
        !body.to_string().contains("secret.txt"),
        "the listing must not leak private content: {body}"
    );

    // The owner sees it.
    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/dirprivate/secret/dir",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "dir listing: {body}");
    assert_eq!(
        entry_names(&body),
        vec!["secret.txt".to_string()],
        "owner listing: {body}"
    );
}

/// A repository that does not exist is a 404.
#[tokio::test]
async fn dir_of_a_missing_repository_is_404() {
    let (app, _guard) = test_app().await;
    let user = register_user(&app, "dirmissing").await;

    let (status, body) = request(
        &app,
        Method::GET,
        "/api/repos/dirmissing/nothing/dir",
        None,
        Some(&user.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "dir listing: {body}");
}
