//! Git repository management and protocol serving for Gita.
//!
//! This crate wraps the pure-Rust `gix` (gitoxide) library to:
//!
//! - create and manage bare repositories
//! - browse repository contents: file trees, commits, file content
//! - serve the smart HTTP protocol via `git http-backend`
//! - compute diffs / merge branches / list new commits via the git CLI
//!
//! All repository *reading* (trees, commits, files) is done in pure Rust
//! via `gix`. Protocol serving, diffs, merges and rev-lists delegate to
//! the battle-tested git CLI.

use anyhow::{anyhow, Context, Result};
use gita_core::TreeEntry;
use gix::Repository;
use std::path::PathBuf;
use std::process::Stdio;

pub mod browse;
pub mod diff_merge;
pub mod refs;
pub mod repo;

/// Manages the on-disk bare repositories and provides git operations.
#[derive(Debug, Clone)]
pub struct GitManager {
    /// Root directory where bare repositories are stored.
    pub root: PathBuf,
}

/// A blob's raw bytes plus the metadata needed to serve them verbatim.
#[derive(Debug, Clone)]
pub struct FileBlob {
    /// The path as requested by the caller (not normalised).
    pub path: String,
    /// Raw, undecoded blob contents. Never lossily converted.
    pub data: Vec<u8>,
    /// Blob object id, as hex.
    pub sha: String,
    /// Blob length in bytes.
    pub size: u64,
}

fn git_cmd() -> std::process::Command {
    std::process::Command::new("git")
}

/// How a user-supplied `reference` string should be interpreted: either a
/// literal object id (a full 40-hex SHA) or the name of a git ref.
enum RefSpec<'a> {
    /// A full 40-hex object id.
    ObjectId(&'a str),
    /// A ref name, already expanded to its fully-qualified form.
    Ref(String),
}

/// Interpret a user-supplied `reference` as either a full 40-hex object id or
/// a ref name.
///
/// Historically every caller assumed a bare name meant `refs/heads/<name>`,
/// which meant a raw commit SHA silently resolved to `refs/heads/<sha>` and
/// failed — so reading a file at an explicit SHA always returned `Ok(None)`.
/// Short-circuiting full SHAs here lets callers pass whichever they have.
fn classify_reference(reference: &str) -> RefSpec<'_> {
    if reference.len() == 40 && reference.bytes().all(|b| b.is_ascii_hexdigit()) {
        return RefSpec::ObjectId(reference);
    }
    RefSpec::Ref(if reference == "HEAD" || reference.contains("refs/") {
        reference.to_string()
    } else {
        format!("refs/heads/{reference}")
    })
}

/// Resolve a ref name to the object id it ultimately points at.
///
/// Symbolic refs (`HEAD`, or a symbolic tag) must be *peeled*: asking gix for
/// `id()` on one panics with "BUG: tries to obtain object id from symbolic
/// target". Callers immediately run `find_commit` on the result, which peels
/// anyway, so peeling here changes nothing for them and removes the panic.
fn resolve_ref_id(repo: &Repository, full: &str) -> Option<gix::ObjectId> {
    let mut reference = repo.find_reference(full).ok()?;
    match reference.peel_to_id() {
        Ok(id) => Some(id.detach()),
        // A ref we found but cannot peel is not a usable target.
        Err(_) => None,
    }
}

fn resolve_reference_id(repo: &Repository, reference: &str) -> Result<gix::ObjectId> {
    match classify_reference(reference) {
        RefSpec::ObjectId(sha) => gix::ObjectId::from_hex(gix::bstr::BStr::new(sha.as_bytes()))
            .map_err(|e| anyhow!("invalid sha {sha}: {e}")),
        RefSpec::Ref(full) => {
            resolve_ref_id(repo, &full).with_context(|| format!("find reference {full}"))
        }
    }
}

/// Like [`resolve_reference_id`], but returns `Ok(None)` when the reference
/// does not exist. This is used by read-only browsing operations so that an
/// empty repository (or a request for a not-yet-created branch) yields an
/// empty result instead of an error/500.
fn resolve_reference_id_opt(repo: &Repository, reference: &str) -> Result<Option<gix::ObjectId>> {
    match classify_reference(reference) {
        RefSpec::ObjectId(sha) => {
            match gix::ObjectId::from_hex(gix::bstr::BStr::new(sha.as_bytes())) {
                Ok(id) => Ok(Some(id)),
                Err(_) => Ok(None),
            }
        }
        RefSpec::Ref(full) => Ok(resolve_ref_id(repo, &full)),
    }
}

/// Maximum directory nesting `collect_tree_entries` will descend. Deeply
/// nested trees are legal in git but pathological; past this depth we emit the
/// directory entry and stop rather than walking an unbounded structure.
const MAX_TREE_DEPTH: usize = 32;

/// Collect every entry at and below `prefix_path`.
///
/// The whole tree below the prefix is flattened into `out`, with each entry's
/// `path` relative to the repository root. Note this is a *recursive* listing:
/// `list_tree(owner, name, ref, "docs")` returns every file under `docs/`,
/// not just `docs`'s immediate children.
fn collect_tree_entries(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix_path: &str,
    out: &mut Vec<TreeEntry>,
) -> Result<()> {
    collect_tree_entries_at(repo, tree, prefix_path, 0, out)
}

fn collect_tree_entries_at(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix_path: &str,
    depth: usize,
    out: &mut Vec<TreeEntry>,
) -> Result<()> {
    for entry in tree.iter() {
        let entry = entry?;
        let is_tree = entry.mode().is_tree();
        let path = if prefix_path.is_empty() {
            entry.filename().to_string()
        } else {
            format!("{prefix_path}/{}", entry.filename())
        };
        out.push(TreeEntry {
            path: path.clone(),
            name: entry.filename().to_string(),
            kind: if is_tree { "tree" } else { "blob" }.to_string(),
            size: None,
        });

        // Descend into directories, stopping at the depth cap. A tree we
        // cannot read (or that is too deep) still appears as an entry above,
        // just without its contents — a truncated listing beats an error.
        if is_tree && depth < MAX_TREE_DEPTH {
            let entry_obj = entry.object()?;
            if entry_obj.kind == gix::object::Kind::Tree {
                if let Ok(subtree) = repo.find_tree(entry_obj.id) {
                    collect_tree_entries_at(repo, &subtree, &path, depth + 1, out)?;
                }
            }
        }
    }
    Ok(())
}

/// Find a tree entry by slash-separated path, descending into subtrees.
fn find_entry_by_path<'a>(
    tree: &'a gix::Tree<'_>,
    path: &str,
) -> Result<gix::object::tree::Entry<'a>> {
    let entry = tree
        .lookup_entry_by_path(path)
        .map_err(|e| anyhow!("lookup path {path}: {e}"))?
        .ok_or_else(|| anyhow!("path not found: {path}"))?;
    Ok(entry)
}

/// Find the start of the CGI body (the byte index just past the header
/// block). `git http-backend` emits `\r\n` line endings, so the separator
/// is `\r\n\r\n`; fall back to `\n\n` for robustness.
fn find_cgi_header_end(bytes: &[u8]) -> Option<usize> {
    if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
        return Some(pos + 4);
    }
    bytes
        .windows(2)
        .position(|w| w == b"\n\n")
        .map(|pos| pos + 2)
}

/// Returns whether `git` is available on PATH.
pub fn git_available() -> bool {
    matches!(git_cmd().arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status(), Ok(s) if s.success())
}

/// A helper to check the git version string.
pub fn git_version() -> String {
    git_cmd()
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "git not found".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a fresh, unique temp root per test invocation.
    ///
    /// `cargo test` runs tests concurrently by default, and the tests below
    /// all create repositories at the same relative path (e.g. `alice/test.git`).
    /// If they shared a single root directory they would race each other:
    /// one test's `create_repo` would see a directory half-created by another
    /// test and fail with `File exists (os error 17)` from gix::init_bare.
    /// Returning a unique directory per call isolates every test.
    fn temp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "gita-git-test-{}-{}-{}",
            std::process::id(),
            nanos,
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn push_initial_commit(mgr: &GitManager, owner: &str, name: &str, repo_path: &std::path::Path) {
        let work = repo_path.parent().unwrap().join("work");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();

        let run = |args: &[&str]| {
            let output = git_cmd()
                .args(args)
                .current_dir(&work)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .expect("git command failed to spawn");
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                panic!(
                    "git command failed: {:?}\nstdout: {}\nstderr: {}",
                    args, stdout, stderr
                );
            }
        };
        run(&["init", "-b", "main"]);
        std::fs::write(work.join("README.md"), "# Hello\n\nThis is a test repo.\n").unwrap();
        std::fs::write(work.join("main.rs"), "fn main() { println!(\"hi\"); }\n").unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "commit",
            "-m",
            "initial commit",
        ]);

        let repo_url = format!("file://{}", repo_path.to_string_lossy());
        eprintln!("Pushing to repo URL: {}", repo_url);
        run(&["remote", "add", "origin", &repo_url]);
        run(&["push", "-u", "origin", "main"]);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn create_and_browse_repo() {
        if !git_available() {
            eprintln!("git not available, skipping");
            return;
        }
        let _ = tracing_subscriber::fmt::try_init();
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo_path =
            rt.block_on(async { mgr.create_repo("alice", "test", "main").await.unwrap() });
        eprintln!("Created repo at: {}", repo_path.display());
        eprintln!("Repo exists: {}", repo_path.exists());
        if repo_path.exists() {
            for entry in std::fs::read_dir(&repo_path).unwrap() {
                eprintln!("  Entry: {:?}", entry.unwrap().file_name());
            }
        }
        push_initial_commit(&mgr, "alice", "test", &repo_path);

        let branches = mgr.list_branches("alice", "test").unwrap();
        assert!(branches.contains(&"main".to_string()));

        let commits = mgr.list_commits("alice", "test", "main", 10).unwrap();
        assert_eq!(commits.len(), 1);
        assert!(commits[0].message.contains("initial commit"));

        let tree = mgr.list_tree("alice", "test", "main", "").unwrap();
        let names: Vec<&str> = tree.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"README.md"));
        assert!(names.contains(&"main.rs"));

        let file = mgr.read_file("alice", "test", "main", "README.md").unwrap();
        assert!(file.is_some());
        assert!(file.unwrap().content.contains("Hello"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A reference that looks like a git *option* must be treated as a
    /// revision, not as an option.
    ///
    /// `git show --output=<file>` writes its output to a file, so a
    /// caller-supplied reference reaching the git command line unchecked
    /// would be an arbitrary-file-write primitive. The `--end-of-options`
    /// marker makes git treat everything after it as a revision.
    #[test]
    fn a_reference_that_looks_like_an_option_is_not_an_option() {
        if !git_available() {
            eprintln!("git not available, skipping");
            return;
        }
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo_path =
            rt.block_on(async { mgr.create_repo("alice", "inject", "main").await.unwrap() });
        push_initial_commit(&mgr, "alice", "inject", &repo_path);

        let victim = root.join("pwned.txt");
        let malicious = format!("--output={}", victim.display());

        // `show_commit` must fail rather than write the file.
        let result = mgr.show_commit("alice", "inject", &malicious);
        assert!(
            result.is_err(),
            "a revision that looks like an option must not be accepted"
        );
        assert!(
            !victim.exists(),
            "the git command line was injectable: it wrote {}",
            victim.display()
        );

        // The same for the diff, which takes two references.
        let victim2 = root.join("pwned2.txt");
        let malicious2 = format!("--output={}", victim2.display());
        let _ = mgr.diff_files_between("alice", "inject", &malicious2, "main");
        assert!(
            !victim2.exists(),
            "the git command line was injectable: it wrote {}",
            victim2.display()
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reference_accepts_full_commit_sha() {
        if !git_available() {
            eprintln!("git not available, skipping");
            return;
        }
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo_path =
            rt.block_on(async { mgr.create_repo("alice", "sha", "main").await.unwrap() });
        push_initial_commit(&mgr, "alice", "sha", &repo_path);

        let sha = mgr.head_sha("alice", "sha", "main").unwrap();
        assert_eq!(sha.len(), 40, "head_sha should be a full 40-hex id");

        // A full SHA must resolve exactly like the branch name does.
        let by_branch = mgr.list_commits("alice", "sha", "main", 10).unwrap();
        let by_sha = mgr.list_commits("alice", "sha", &sha, 10).unwrap();
        assert_eq!(by_branch.len(), 1);
        assert_eq!(
            by_sha.len(),
            1,
            "a full SHA should resolve; it previously fell through to refs/heads/<sha> and yielded nothing"
        );
        assert_eq!(by_sha[0].sha, by_branch[0].sha);

        // Reading a file at the SHA works too.
        let file = mgr.read_file("alice", "sha", &sha, "README.md").unwrap();
        assert!(file.is_some(), "read_file should accept a full SHA");
        assert!(file.unwrap().content.contains("Hello"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reference_rejects_malformed_sha_without_erroring() {
        if !git_available() {
            eprintln!("git not available, skipping");
            return;
        }
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo_path =
            rt.block_on(async { mgr.create_repo("alice", "badsha", "main").await.unwrap() });
        push_initial_commit(&mgr, "alice", "badsha", &repo_path);

        // 40 chars but not hex: must degrade to an empty result, not panic or error.
        let bogus = "z".repeat(40);
        assert!(
            mgr.list_commits("alice", "badsha", &bogus, 10)
                .unwrap()
                .is_empty(),
            "malformed sha should yield no commits"
        );
        assert!(
            mgr.read_file("alice", "badsha", &bogus, "README.md")
                .unwrap()
                .is_none(),
            "read_file at a malformed sha should return None"
        );

        // A well-formed SHA that simply doesn't exist in this repo.
        let absent = "0".repeat(40);
        assert!(mgr
            .list_commits("alice", "badsha", &absent, 10)
            .unwrap()
            .is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn classify_reference_treats_only_full_hex_shas_as_object_ids() {
        // Branch-ish names keep the refs/heads prefix.
        assert!(matches!(
            classify_reference("main"),
            RefSpec::Ref(r) if r == "refs/heads/main"
        ));
        assert!(matches!(
            classify_reference("feature/x"),
            RefSpec::Ref(r) if r == "refs/heads/feature/x"
        ));
        assert!(matches!(
            classify_reference("HEAD"),
            RefSpec::Ref(r) if r == "HEAD"
        ));
        assert!(matches!(
            classify_reference("refs/tags/v1"),
            RefSpec::Ref(r) if r == "refs/tags/v1"
        ));

        // Abbreviated SHAs are NOT object ids — too short to be unambiguous.
        assert!(matches!(
            classify_reference("abc1234"),
            RefSpec::Ref(r) if r == "refs/heads/abc1234"
        ));
        // 40 hex chars is an object id.
        assert!(matches!(
            classify_reference(&"a".repeat(40)),
            RefSpec::ObjectId(_)
        ));
        // 40 chars with a non-hex byte is not.
        assert!(matches!(
            classify_reference(&format!("{}z", "a".repeat(39))),
            RefSpec::Ref(_)
        ));
    }

    #[tokio::test]
    async fn repo_path_concatenation() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let p = mgr.repo_path("alice", "widgets");
        assert_eq!(p, root.join("alice").join("widgets.git"));
    }

    /// The owner segment is a directory name, so a value that could escape
    /// the repositories root must be rejected by the mutating operations.
    #[test]
    fn safe_repo_path_rejects_traversal() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());

        // Ordinary names are fine.
        assert!(mgr.safe_repo_path("alice", "widgets").is_ok());
        assert!(mgr.safe_repo_path("my-org", "my.repo").is_ok());

        // Anything that could escape the root is rejected.
        for (owner, name) in [
            ("..", "widgets"),
            ("../..", "widgets"),
            ("alice", ".."),
            ("alice/../..", "widgets"),
            ("alice", "../../etc"),
            ("alice", "..\\.."),
            ("", "widgets"),
            ("alice", ""),
            (".", "widgets"),
            ("alice", "."),
        ] {
            let err = mgr
                .safe_repo_path(owner, name)
                .expect_err("should reject {owner}/{name}");
            assert!(
                err.to_string().contains("invalid") || err.to_string().contains("empty"),
                "unexpected error for {owner}/{name}: {err}"
            );
        }
    }

    /// A traversal attempt must not create anything outside the root.
    #[tokio::test]
    async fn create_repo_rejects_traversal_owner() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let err = mgr
            .create_repo("../escaped", "pwn", "main")
            .await
            .expect_err("traversal owner must be rejected");
        assert!(err.to_string().contains("invalid"), "got: {err}");
        // Nothing was created outside the repositories root.
        assert!(
            !root.parent().unwrap().join("escaped.git").exists(),
            "a directory was created outside the repositories root"
        );
    }

    #[tokio::test]
    async fn create_repo_then_repo_exists() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        assert!(
            !mgr.repo_exists("alice", "test"),
            "fresh repo should not exist"
        );
        mgr.create_repo("alice", "test", "main").await.unwrap();
        assert!(mgr.repo_exists("alice", "test"));
    }

    #[tokio::test]
    async fn create_repo_duplicate_is_an_error() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        mgr.create_repo("alice", "test", "main").await.unwrap();
        let err = mgr.create_repo("alice", "test", "main").await.unwrap_err();
        assert!(err.to_string().contains("repository already exists"));
    }

    #[tokio::test]
    async fn delete_repo_makes_repo_missing() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        mgr.create_repo("alice", "test", "main").await.unwrap();
        assert!(mgr.repo_exists("alice", "test"));
        mgr.delete_repo("alice", "test").unwrap();
        assert!(!mgr.repo_exists("alice", "test"));
    }

    #[tokio::test]
    async fn default_branch_reflects_what_we_set() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        mgr.create_repo("alice", "test", "develop").await.unwrap();
        let head = mgr.default_branch("alice", "test").unwrap();
        assert_eq!(head, "develop");
    }

    #[tokio::test]
    async fn read_file_missing_returns_none() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        mgr.create_repo("alice", "test", "main").await.unwrap();
        push_initial_commit(&mgr, "alice", "test", &mgr.repo_path("alice", "test"));
        let res = mgr
            .read_file("alice", "test", "main", "does-not-exist.txt")
            .unwrap();
        assert!(
            res.is_none(),
            "missing file should return Ok(None), got {res:?}"
        );
    }

    #[tokio::test]
    async fn list_tree_descends_into_subtrees() {
        if !git_available() {
            eprintln!("git not available, skipping");
            return;
        }
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let path = mgr.create_repo("alice", "deep", "main").await.unwrap();
        push_initial_commit(&mgr, "alice", "deep", &path);
        push_branch(
            &mgr,
            "alice",
            "deep",
            "main",
            &[
                ("docs/index.html", "<h1>hi</h1>\n"),
                ("docs/a/b/deep.txt", "buried\n"),
            ],
            "add nested docs",
        );

        // The whole point: files several levels down must appear. Previously
        // collect_tree_entries never descended, so only top-level names showed.
        let entries = mgr.list_tree("alice", "deep", "main", "").unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(
            paths.contains(&"docs/index.html"),
            "expected docs/index.html in {paths:?}"
        );
        assert!(
            paths.contains(&"docs/a/b/deep.txt"),
            "expected a nested file in {paths:?}"
        );

        // The intermediate directories appear as tree entries too.
        let doc_dirs: Vec<&TreeEntry> = entries.iter().filter(|e| e.kind == "tree").collect();
        assert!(
            doc_dirs.iter().any(|e| e.path == "docs"),
            "expected a tree entry for docs"
        );
        assert!(
            doc_dirs.iter().any(|e| e.path == "docs/a"),
            "expected a tree entry for docs/a"
        );

        // Scoping to a prefix returns that subtree's contents.
        let scoped = mgr.list_tree("alice", "deep", "main", "docs").unwrap();
        let scoped_paths: Vec<&str> = scoped.iter().map(|e| e.path.as_str()).collect();
        assert!(
            scoped_paths.contains(&"docs/index.html"),
            "scoped listing should include docs/index.html, got {scoped_paths:?}"
        );
        assert!(
            scoped_paths.contains(&"docs/a/b/deep.txt"),
            "scoped listing should include nested files, got {scoped_paths:?}"
        );
        assert!(
            !scoped_paths.contains(&"README.md"),
            "scoped listing must not leak files outside the prefix"
        );

        // A path that is a file (not a directory) yields an empty listing.
        assert!(mgr
            .list_tree("alice", "deep", "main", "README.md")
            .unwrap()
            .is_empty());
        // A path that does not exist yields an empty listing.
        assert!(mgr
            .list_tree("alice", "deep", "main", "nope")
            .unwrap()
            .is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A real 1x1 PNG. Chosen because it contains bytes that are invalid
    /// UTF-8, so any lossy decode mangles it visibly.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x18, 0x00, 0x00, 0x00, 0x26, 0x08, 0x02, 0x00, 0x00, 0x00, 0x39,
        0x1D, 0x09, 0x7C, 0x11, 0xFF, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C,
        0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[tokio::test]
    async fn read_file_bytes_roundtrips_binary_content() {
        if !git_available() {
            eprintln!("git not available, skipping");
            return;
        }
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let path = mgr.create_repo("alice", "bin", "main").await.unwrap();
        push_initial_commit(&mgr, "alice", "bin", &path);
        push_branch_bytes(
            &mgr,
            "alice",
            "bin",
            "main",
            &[
                ("assets/logo.png", TINY_PNG),
                ("index.html", b"<h1>hi</h1>\n"),
            ],
            "add binary asset",
        );

        let blob = mgr
            .read_file_bytes("alice", "bin", "main", "assets/logo.png")
            .unwrap()
            .expect("png should be found");

        // Byte-exact: this is the guarantee a lossless read exists to provide.
        assert_eq!(
            blob.data, TINY_PNG,
            "binary content must round-trip exactly"
        );
        assert_eq!(blob.size as usize, TINY_PNG.len());
        assert_eq!(blob.size, blob.data.len() as u64);
        assert_eq!(blob.path, "assets/logo.png");
        assert_eq!(blob.sha.len(), 40, "blob sha should be a full hex id");

        // The lossy text path mangles it, which is exactly why this method
        // exists. Assert the difference rather than just trusting it.
        let lossy = mgr
            .read_file("alice", "bin", "main", "assets/logo.png")
            .unwrap()
            .expect("read_file still returns the blob");
        assert_ne!(
            lossy.content.as_bytes(),
            TINY_PNG,
            "read_file is lossy by design; read_file_bytes must not be"
        );

        // Text files come through unchanged on both paths.
        let html = mgr
            .read_file_bytes("alice", "bin", "main", "index.html")
            .unwrap()
            .unwrap();
        assert_eq!(html.data, b"<h1>hi</h1>\n");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn read_file_bytes_returns_none_for_missing_and_directory_paths() {
        if !git_available() {
            eprintln!("git not available, skipping");
            return;
        }
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let path = mgr.create_repo("alice", "bytes", "main").await.unwrap();
        push_initial_commit(&mgr, "alice", "bytes", &path);
        push_branch_bytes(
            &mgr,
            "alice",
            "bytes",
            "main",
            &[("docs/index.html", b"<h1>hi</h1>\n")],
            "add docs",
        );

        // Missing file.
        assert!(
            mgr.read_file_bytes("alice", "bytes", "main", "nope.png")
                .unwrap()
                .is_none(),
            "missing file should be None"
        );
        // A directory resolves to a tree entry; that is not file content and
        // must not blow up in `into_blob`.
        assert!(
            mgr.read_file_bytes("alice", "bytes", "main", "docs")
                .unwrap()
                .is_none(),
            "a directory path should be None, not a panic"
        );
        // Same for read_file, which previously would have decoded a tree.
        assert!(
            mgr.read_file("alice", "bytes", "main", "docs")
                .unwrap()
                .is_none(),
            "read_file on a directory should be None"
        );
        // Missing reference.
        assert!(mgr
            .read_file_bytes("alice", "bytes", "no-such-branch", "docs/index.html")
            .unwrap()
            .is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn ref_exists_reports_branch_presence() {
        if !git_available() {
            eprintln!("git not available, skipping");
            return;
        }
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let path = mgr.create_repo("alice", "refs", "main").await.unwrap();
        push_initial_commit(&mgr, "alice", "refs", &path);

        assert!(mgr.ref_exists("alice", "refs", "main"));
        assert!(mgr.ref_exists("alice", "refs", "HEAD"));
        assert!(!mgr.ref_exists("alice", "refs", "no-such-branch"));
        assert!(!mgr.ref_exists("alice", "refs", "refs/heads/nope"));
        // A well-formed but absent SHA is not a commit.
        assert!(!mgr.ref_exists("alice", "refs", &"0".repeat(40)));
        // An unknown repository must not panic.
        assert!(!mgr.ref_exists("alice", "no-such-repo", "main"));

        // A real commit sha does exist.
        let sha = mgr.head_sha("alice", "refs", "main").unwrap();
        assert!(mgr.ref_exists("alice", "refs", &sha));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn head_sha_matches_pushed_commit() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let path = mgr.create_repo("alice", "test", "main").await.unwrap();
        push_initial_commit(&mgr, "alice", "test", &path);
        let sha = mgr.head_sha("alice", "test", "main").unwrap();
        assert!(!sha.is_empty(), "head sha should not be empty");
        // Create a second commit and verify head_sha advances.
        let work = path.parent().unwrap().join("work2");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let run = |args: &[&str]| {
            let out = git_cmd()
                .args(args)
                .current_dir(&work)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "t@e.io")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "t@e.io")
                .output()
                .expect("git");
            assert!(out.status.success());
        };
        let bare_url = format!("file://{}", path.display());
        run(&["clone", "-q", &bare_url, "."]);
        std::fs::write(work.join("next.md"), "next").unwrap();
        run(&["add", "next.md"]);
        run(&["commit", "-q", "-m", "second"]);
        run(&["push", "-q", "origin", "main"]);
        let sha2 = mgr.head_sha("alice", "test", "main").unwrap();
        assert_ne!(sha, sha2, "head_sha must advance after a push");
    }

    /// A merge must keep the changes the *target* branch made on its own.
    ///
    /// The merge commit's tree used to be the head commit's tree, which
    /// silently reverted every file the target branch had changed but the
    /// source branch had not. "No conflicts" means the two sides did not
    /// touch the same lines — it does not mean the head tree already
    /// contains the target's work.
    #[tokio::test]
    async fn merge_keeps_target_branch_only_changes() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let path = mgr.create_repo("alice", "mergetest", "main").await.unwrap();
        push_initial_commit(&mgr, "alice", "mergetest", &path);

        let work = path.parent().unwrap().join("mergework");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let run = |args: &[&str]| {
            let out = git_cmd()
                .args(args)
                .current_dir(&work)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "t@e.io")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "t@e.io")
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?} failed");
        };
        let bare_url = format!("file://{}", path.display());
        run(&["clone", "-q", &bare_url, "."]);

        // A shared base file, so the two branches are genuinely related.
        std::fs::write(work.join("shared.txt"), "base").unwrap();
        run(&["add", "shared.txt"]);
        run(&["commit", "-q", "-m", "base"]);
        run(&["push", "-q", "origin", "main"]);

        // The feature branch adds a file of its own.
        run(&["checkout", "-q", "-b", "feature"]);
        std::fs::write(work.join("only_feature.txt"), "from feature").unwrap();
        run(&["add", "only_feature.txt"]);
        run(&["commit", "-q", "-m", "feature change"]);
        run(&["push", "-q", "origin", "feature"]);
        let head_sha = mgr.head_sha("alice", "mergetest", "feature").unwrap();

        // The target branch adds a *different* file of its own.
        run(&["checkout", "-q", "main"]);
        std::fs::write(work.join("only_main.txt"), "from main").unwrap();
        run(&["add", "only_main.txt"]);
        run(&["commit", "-q", "-m", "main change"]);
        run(&["push", "-q", "origin", "main"]);

        // No conflicts: the two sides touched different files.
        let conflicts = mgr
            .check_merge_conflict("alice", "mergetest", "main", &head_sha)
            .unwrap();
        assert!(
            conflicts.is_empty(),
            "expected a clean merge, got {conflicts:?}"
        );

        mgr.merge_branch("alice", "mergetest", "main", &head_sha, "Test", "t@e.io")
            .unwrap();

        // The merged tree must contain BOTH sides' files.
        let entries = mgr.list_tree("alice", "mergetest", "main", "").unwrap();
        let paths: Vec<String> = entries.into_iter().map(|e| e.path).collect();
        assert!(
            paths.contains(&"only_feature.txt".to_string()),
            "source branch's file missing after merge: {paths:?}"
        );
        assert!(
            paths.contains(&"only_main.txt".to_string()),
            "target branch's own change was dropped by the merge: {paths:?}"
        );
        assert!(
            paths.contains(&"shared.txt".to_string()),
            "base file missing after merge: {paths:?}"
        );
    }

    #[tokio::test]
    async fn branch_and_tag_crud() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let path = mgr.create_repo("alice", "branchtag", "main").await.unwrap();
        push_initial_commit(&mgr, "alice", "branchtag", &path);

        // Create a branch from HEAD.
        mgr.create_branch("alice", "branchtag", "feature", None)
            .unwrap();
        let branches = mgr.list_branches("alice", "branchtag").unwrap();
        assert!(branches.contains(&"feature".to_string()));

        // Create a branch from a specific sha (main head).
        let main_sha = mgr.head_sha("alice", "branchtag", "main").unwrap();
        mgr.create_branch("alice", "branchtag", "from-sha", Some(&main_sha))
            .unwrap();
        let branches = mgr.list_branches("alice", "branchtag").unwrap();
        assert!(branches.contains(&"from-sha".to_string()));

        // Duplicate branch -> error.
        let err = mgr
            .create_branch("alice", "branchtag", "feature", None)
            .unwrap_err();
        assert!(err.to_string().contains("already exists"));

        // Delete branch.
        mgr.delete_branch("alice", "branchtag", "feature").unwrap();
        let branches = mgr.list_branches("alice", "branchtag").unwrap();
        assert!(!branches.contains(&"feature".to_string()));

        // Tags.
        mgr.create_tag("alice", "branchtag", "v1.0.0", Some(&main_sha))
            .unwrap();
        let tags = mgr.list_tags("alice", "branchtag").unwrap();
        assert!(tags.contains(&"v1.0.0".to_string()));

        // Duplicate tag -> error.
        let err = mgr
            .create_tag("alice", "branchtag", "v1.0.0", None)
            .unwrap_err();
        assert!(err.to_string().contains("already exists"));

        mgr.delete_tag("alice", "branchtag", "v1.0.0").unwrap();
        let tags = mgr.list_tags("alice", "branchtag").unwrap();
        assert!(!tags.contains(&"v1.0.0".to_string()));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn rename_repo_moves_bare_dir() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        mgr.create_repo("alice", "before", "main").await.unwrap();
        assert!(mgr.repo_exists("alice", "before"));

        // Rename within the same namespace.
        mgr.rename_repo("alice", "before", "alice", "after")
            .unwrap();
        assert!(!mgr.repo_exists("alice", "before"));
        assert!(mgr.repo_exists("alice", "after"));

        // Rename to a different namespace.
        mgr.rename_repo("alice", "after", "bob", "shared").unwrap();
        assert!(!mgr.repo_exists("alice", "after"));
        assert!(mgr.repo_exists("bob", "shared"));

        // Rename to existing target -> error.
        mgr.create_repo("alice", "collision", "main").await.unwrap();
        let err = mgr
            .rename_repo("bob", "shared", "alice", "collision")
            .unwrap_err();
        assert!(err.to_string().contains("already exists"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn git_available_and_version_helpers() {
        // These should not panic and should at least identify whether
        // git is present.
        let _ = git_available();
        let v = git_version();
        if git_available() {
            assert!(v.contains("git version"));
        } else {
            assert_eq!(v, "git not found");
        }
    }

    /// Push a branch with the given file contents onto the bare repo.
    /// Creates a worktree, writes files, commits and pushes.
    fn push_branch(
        mgr: &GitManager,
        owner: &str,
        name: &str,
        branch: &str,
        files: &[(&str, &str)],
        message: &str,
    ) {
        let owned: Vec<(&str, &[u8])> = files.iter().map(|(k, v)| (*k, v.as_bytes())).collect();
        push_branch_bytes(mgr, owner, name, branch, &owned, message)
    }

    /// Write `files` into the work tree at `work`, creating parent
    /// directories so nested paths like `docs/a/b.html` work.
    fn push_branch_files_into(work: &std::path::Path, files: &[(&str, &[u8])]) {
        for (file, content) in files {
            let dest = work.join(file);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&dest, content).unwrap();
        }
    }

    /// Byte-capable sibling of `push_branch`: writes raw file contents, so
    /// tests can commit non-UTF-8 blobs (images, fonts, wasm).
    fn push_branch_bytes(
        mgr: &GitManager,
        owner: &str,
        name: &str,
        branch: &str,
        files: &[(&str, &[u8])],
        message: &str,
    ) {
        let path = mgr.repo_path(owner, name);
        let work = path.parent().unwrap().join(format!("work-{branch}"));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let run = |args: &[&str]| {
            let out = git_cmd()
                .args(args)
                .current_dir(&work)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "t@e.io")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "t@e.io")
                .output()
                .expect("git");
            if !out.status.success() {
                panic!(
                    "git {:?} failed: {}",
                    args,
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        };
        let bare_url = format!("file://{}", path.display());
        run(&["clone", "-q", "-b", "main", &bare_url, "."]);
        push_branch_files_into(&work, files);
        if branch != "main" {
            run(&["checkout", "-q", "-b", branch]);
        }
        run(&["add", "."]);
        run(&["commit", "-q", "-m", message]);
        run(&["push", "-q", "-u", "origin", branch]);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[tokio::test]
    async fn merge_fast_forwards_when_head_is_descendant() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let path = mgr.create_repo("alice", "ff", "main").await.unwrap();
        push_initial_commit(&mgr, "alice", "ff", &path);

        // Create a feature branch that extends main.
        push_branch(
            &mgr,
            "alice",
            "ff",
            "feature",
            &[("feature.txt", "x\n")],
            "feature work",
        );
        let main_before = mgr.head_sha("alice", "ff", "main").unwrap();
        let feature_sha = mgr.head_sha("alice", "ff", "feature").unwrap();

        // Fast-forward: feature is a descendant of main.
        assert!(mgr
            .is_ancestor("alice", "ff", &main_before, &feature_sha)
            .unwrap());

        // Merge should advance main to feature sha.
        mgr.merge_branch(
            "alice",
            "ff",
            "main",
            &feature_sha,
            "Test",
            "test@example.com",
        )
        .unwrap();
        let main_after = mgr.head_sha("alice", "ff", "main").unwrap();
        assert_eq!(main_after, feature_sha);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn merge_creates_merge_commit_when_branches_diverge() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let path = mgr.create_repo("alice", "merge", "main").await.unwrap();
        push_initial_commit(&mgr, "alice", "merge", &path);
        let main_before = mgr.head_sha("alice", "merge", "main").unwrap();

        // Branch feature from main, add a file.
        push_branch(
            &mgr,
            "alice",
            "merge",
            "feature",
            &[("feat.md", "feature\n")],
            "feat",
        );
        let feature_sha = mgr.head_sha("alice", "merge", "feature").unwrap();

        // Advance main independently (different file -> no conflict).
        push_branch(
            &mgr,
            "alice",
            "merge",
            "main",
            &[("main.md", "main\n")],
            "main work",
        );
        let main_diverged = mgr.head_sha("alice", "merge", "main").unwrap();
        assert_ne!(main_diverged, feature_sha);

        // No conflict expected.
        let conflicts = mgr
            .check_merge_conflict("alice", "merge", "main", &feature_sha)
            .unwrap();
        assert!(
            conflicts.is_empty(),
            "expected no conflict, got {conflicts:?}"
        );

        // Merge should create a merge commit (not fast-forward).
        mgr.merge_branch(
            "alice",
            "merge",
            "main",
            &feature_sha,
            "Test",
            "test@example.com",
        )
        .unwrap();
        let main_after = mgr.head_sha("alice", "merge", "main").unwrap();
        assert_ne!(
            main_after, feature_sha,
            "merge commit should not equal feature head"
        );
        assert_ne!(main_after, main_diverged, "main should advance");
        // The new head should have two parents (merge commit).
        let parents = mgr
            .open("alice", "merge")
            .unwrap()
            .find_commit(
                gix::ObjectId::from_hex(gix::bstr::BStr::new(main_after.as_bytes())).unwrap(),
            )
            .unwrap()
            .parent_ids()
            .count();
        assert_eq!(parents, 2, "merge commit must have two parents");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn merge_conflict_detected_and_merge_rejected() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let path = mgr.create_repo("alice", "conflict", "main").await.unwrap();
        push_initial_commit(&mgr, "alice", "conflict", &path);

        // Feature modifies README.md.
        push_branch(
            &mgr,
            "alice",
            "conflict",
            "feature",
            &[("README.md", "# Feature\n")],
            "feat readme",
        );
        let feature_sha = mgr.head_sha("alice", "conflict", "feature").unwrap();

        // Main also modifies README.md differently.
        push_branch(
            &mgr,
            "alice",
            "conflict",
            "main",
            &[("README.md", "# Main\n")],
            "main readme",
        );
        let main_sha = mgr.head_sha("alice", "conflict", "main").unwrap();
        assert!(!mgr
            .is_ancestor("alice", "conflict", &main_sha, &feature_sha)
            .unwrap());

        // Conflict detection should report README.md.
        let conflicts = mgr
            .check_merge_conflict("alice", "conflict", "main", &feature_sha)
            .unwrap();
        assert!(
            conflicts.iter().any(|p| p == "README.md"),
            "expected README.md conflict, got {conflicts:?}"
        );

        // merge_branch must fail with a conflict error and leave main untouched.
        let err = mgr
            .merge_branch(
                "alice",
                "conflict",
                "main",
                &feature_sha,
                "Test",
                "test@example.com",
            )
            .unwrap_err();
        assert!(err.to_string().contains("merge conflict"), "got: {err}");
        let main_after = mgr.head_sha("alice", "conflict", "main").unwrap();
        assert_eq!(main_after, main_sha, "main must not change on conflict");

        let _ = std::fs::remove_dir_all(&root);
    }
}
