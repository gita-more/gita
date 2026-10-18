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
use gita_core::{Commit, FileContent, TreeEntry};
use gix::Repository;
use std::path::{PathBuf};
use std::process::Stdio;
use tracing::debug;

/// Manages the on-disk bare repositories and provides git operations.
#[derive(Debug, Clone)]
pub struct GitManager {
    /// Root directory where bare repositories are stored.
    pub root: PathBuf,
}

impl GitManager {
    /// Create a new manager rooted at `root`.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The filesystem path of a repository with the given owner/name.
    pub fn repo_path(&self, owner: &str, name: &str) -> PathBuf {
        self.root.join(owner).join(format!("{name}.git"))
    }

    /// Initialize a new bare repository.
    pub async fn create_repo(&self, owner: &str, name: &str, default_branch: &str) -> Result<PathBuf> {
        let path = self.repo_path(owner, name);
        if path.exists() {
            return Err(anyhow!("repository already exists: {owner}/{name}"));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create parent dir {}", parent.display()))?;
        }
        let _repo = gix::init_bare(&path)
            .map_err(|e| anyhow!("init bare repo {}: {e}", path.display()))?;
        // Set the default branch for fresh repositories.
        let _ = git_cmd()
            .arg("--git-dir")
            .arg(&path)
            .args(["symbolic-ref", "HEAD", &format!("refs/heads/{default_branch}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        debug!("created bare repo at {}", path.display());
        Ok(path)
    }

    /// Check whether a repository exists on disk.
    pub fn repo_exists(&self, owner: &str, name: &str) -> bool {
        self.repo_path(owner, name).exists()
    }

    /// Delete a repository directory (used when a repo is removed).
    pub fn delete_repo(&self, owner: &str, name: &str) -> Result<()> {
        let path = self.repo_path(owner, name);
        if path.exists() {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("remove repo dir {}", path.display()))?;
        }
        Ok(())
    }

    /// Open a bare repository with gix.
    pub fn open(&self, owner: &str, name: &str) -> Result<Repository> {
        let path = self.repo_path(owner, name);
        gix::open(&path)
            .with_context(|| format!("open bare repo {}", path.display()))
    }

    /// Run `git http-backend` as a CGI subprocess to serve a smart HTTP request.
    pub fn run_http_backend(
        &self,
        method: &str,
        path_info: &str,
        query_string: Option<&str>,
        content_type: &str,
        body: &[u8],
    ) -> Result<(u16, Vec<(String, String)>, Vec<u8>)> {
        let mut cmd = git_cmd();
        cmd.arg("http-backend");
        cmd.env("GIT_PROJECT_ROOT", &self.root);
        cmd.env("GIT_HTTP_EXPORT_ALL", "1");
        cmd.env("PATH_INFO", path_info);
        cmd.env("REQUEST_METHOD", method);
        // Note: do not set GIT_PROTOCOL here. When it is set to
        // `version=2`, `git http-backend` serves receive-pack (push)
        // advertisements without the `# service=git-receive-pack`
        // preamble line, which git clients reject with
        // "not valid: is this a git repository?". Leaving it unset lets
        // http-backend negotiate the protocol version with the client.
        if let Some(qs) = query_string {
            cmd.env("QUERY_STRING", qs);
        }
        cmd.env("CONTENT_TYPE", content_type);
        cmd.env("CONTENT_LENGTH", body.len().to_string());
        // Enable the receive-pack (push) service. `git http-backend` only
        // serves receive-pack when the repository config sets
        // `http.receivepack = true`; inject that setting per-request so
        // push works for every repository without mutating on-disk configs.
        cmd.env("GIT_CONFIG_COUNT", "1");
        cmd.env("GIT_CONFIG_KEY_0", "http.receivepack");
        cmd.env("GIT_CONFIG_VALUE_0", "true");
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow!("failed to spawn git http-backend: {e}"))?;

        // Write body to stdin.
        {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(body)?;
            }
        }

        let output = child
            .wait_with_output()
            .map_err(|e| anyhow!("git http-backend failed: {e}"))?;

        if !output.stderr.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!("http-backend stderr: {}", stderr.trim());
        }

        let stdout = output.stdout;
        let mut status: u16 = 200;
        let mut headers = Vec::new();
        let mut body: &[u8] = &stdout;

        // Parse status line and headers from the CGI output.
        if let Some(body_start) = find_cgi_header_end(&stdout) {
            let header_text = String::from_utf8_lossy(&stdout[..body_start]);
            for line in header_text.lines() {
                let line = line.trim_end_matches('\r');
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim().to_string();
                    let value = value.trim().to_string();
                    if key.eq_ignore_ascii_case("Status") {
                        if let Some(code) = value.split_whitespace().next() {
                            if let Ok(c) = code.parse::<u16>() {
                                status = c;
                            }
                        }
                    } else {
                        headers.push((key, value));
                    }
                }
            }
            body = &stdout[body_start..];
        }

        Ok((status, headers, body.to_vec()))
    }

    /// List refs (branches) of a repository.
    pub fn list_branches(&self, owner: &str, name: &str) -> Result<Vec<String>> {
        let repo = self.open(owner, name)?;
        let mut branches = Vec::new();
        for reference in repo.references()?.all()? {
            let name = reference?.name().to_string();
            if let Some(head) = name.strip_prefix("refs/heads/") {
                branches.push(head.to_string());
            }
        }
        branches.sort();
        Ok(branches)
    }

    /// Resolve a branch name (or HEAD) to a commit SHA.
    pub fn resolve_branch(&self, owner: &str, name: &str, branch: &str) -> Result<String> {
        let repo = self.open(owner, name)?;
        let full_name = if branch == "HEAD" || branch.contains("refs/") {
            branch.to_string()
        } else {
            format!("refs/heads/{branch}")
        };
        let reference = repo
            .find_reference(&full_name)
            .with_context(|| format!("find reference {full_name}"))?;
        Ok(reference.id().to_string())
    }

    /// The default branch (HEAD symbolic ref) of a repository.
    pub fn default_branch(&self, owner: &str, name: &str) -> Result<String> {
        let repo = self.open(owner, name)?;
        // Try branch_names() (available in gix 0.89) and pick the one matching HEAD.
        if let Ok(head) = repo.head() {
            if let Ok(head_id) = head.into_peeled_id() {
                let branch_names = repo.branch_names();
                for name in branch_names {
                    let Ok(full) = gix::refs::FullName::try_from(format!("refs/heads/{name}")) else {
                        continue;
                    };
                    if let Ok(reference) = repo.find_reference(&full) {
                        if reference.id() == head_id {
                            return Ok(name.to_string());
                        }
                    }
                }
            }
        }
        let branches = self.list_branches(owner, name)?;
        if branches.contains(&"main".to_string()) {
            Ok("main".to_string())
        } else if branches.contains(&"master".to_string()) {
            Ok("master".to_string())
        } else {
            Ok(branches.first().cloned().unwrap_or_else(|| "main".to_string()))
        }
    }

    /// List files in a directory of a repository at a given ref.
    pub fn list_tree(
        &self,
        owner: &str,
        name: &str,
        reference: &str,
        path: &str,
    ) -> Result<Vec<TreeEntry>> {
        let repo = self.open(owner, name)?;
        let Some(id) = resolve_reference_id_opt(&repo, reference)? else {
            return Ok(Vec::new());
        };
        let tree = repo.find_commit(id)?.tree()?;
        let mut entries = Vec::new();
        collect_tree_entries(&tree, path, &mut entries)?;
        Ok(entries)
    }

    /// Read a file's content at a given ref and path.
    pub fn read_file(
        &self,
        owner: &str,
        name: &str,
        reference: &str,
        path: &str,
    ) -> Result<Option<FileContent>> {
        let repo = self.open(owner, name)?;
        let Some(id) = resolve_reference_id_opt(&repo, reference)? else {
            return Ok(None);
        };
        let tree = repo.find_commit(id)?.tree()?;
        let entry = match find_entry_by_path(&tree, path) {
            Ok(e) => e,
            Err(_) => return Ok(None),
        };
        let object = entry.object()?.into_blob();
        let content = String::from_utf8_lossy(&object.data).to_string();
        Ok(Some(FileContent {
            path: path.to_string(),
            content,
            sha: entry.oid().to_string(),
        }))
    }

    /// List recent commits of a repository, newest first.
    pub fn list_commits(
        &self,
        owner: &str,
        name: &str,
        reference: &str,
        limit: usize,
    ) -> Result<Vec<Commit>> {
        let repo = self.open(owner, name)?;
        let Some(id) = resolve_reference_id_opt(&repo, reference)? else {
            return Ok(Vec::new());
        };
        let walk = repo
            .rev_walk([id])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .selected(|_| true)?;
        let mut commits = Vec::new();
        for info in walk.take(limit) {
            let commit = info?.object()?;
            let message = commit.message_raw_sloppy().to_string();
            let author = commit.author()?;
            let committer = commit.committer()?;
            commits.push(Commit {
                id: 0,
                repo_id: 0,
                sha: commit.id.to_string(),
                author_name: author.name.to_string(),
                author_email: author.email.to_string(),
                message: message.trim().to_string(),
                parent_shas: commit.parent_ids().map(|p| p.to_string()).collect(),
                authored_at: chrono::DateTime::from_timestamp(
                    committer.seconds(),
                    0,
                )
                .unwrap_or_else(chrono::Utc::now),
            });
        }
        Ok(commits)
    }

    /// Compute a unified diff between two refs using the git CLI.
    pub fn diff_between(
        &self,
        owner: &str,
        name: &str,
        base_ref: &str,
        head_ref: &str,
    ) -> Result<String> {
        let path = self.repo_path(owner, name);
        let output = git_cmd()
            .args(["--git-dir"])
            .arg(&path)
            .args(["diff", "--no-color", "--no-ext-diff", base_ref, head_ref, "--"])
            .output()
            .map_err(|e| anyhow!("git diff failed: {e}"))?;
        if !output.status.success() {
            return Err(anyhow!(
                "git diff error: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// List commit SHAs reachable from `head` but not from `base`.
    pub fn new_commit_shas(
        &self,
        owner: &str,
        name: &str,
        base_ref: &str,
        head_ref: &str,
    ) -> Result<Vec<String>> {
        let path = self.repo_path(owner, name);
        let output = git_cmd()
            .args(["--git-dir"])
            .arg(&path)
            .args(["rev-list", "--reverse", &format!("{base_ref}..{head_ref}")])
            .output()
            .map_err(|e| anyhow!("git rev-list failed: {e}"))?;
        if !output.status.success() {
            return Err(anyhow!(
                "git rev-list error: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(text
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }

    /// Get the SHA of the latest commit on a branch.
    pub fn head_sha(&self, owner: &str, name: &str, reference: &str) -> Result<String> {
        let repo = self.open(owner, name)?;
        let id = resolve_reference_id(&repo, reference)?;
        Ok(id.to_string())
    }

    /// Merge a source commit into a target branch (fast-forward).
    pub fn merge_branch(
        &self,
        owner: &str,
        name: &str,
        target_branch: &str,
        head_sha: &str,
    ) -> Result<()> {
        let repo = self.open(owner, name)?;
        let full_target = format!("refs/heads/{target_branch}");
        let target = repo
            .find_reference(&full_target)
            .with_context(|| format!("find target branch {full_target}"))?;

        // Validate the head sha.
        let head_id = gix::ObjectId::from_hex(gix::bstr::BStr::new(head_sha.as_bytes()))
            .map_err(|e| anyhow!("invalid head sha {head_sha}: {e}"))?;
        let _ = repo.find_commit(head_id)?;

        // Build a reference edit for the fast-forward update.
        // The previous value must match the currently resolved target id.
        let prev_target = target.id().detach();
        let edit = gix::refs::transaction::RefEdit::update(
            gix::refs::FullName::try_from(full_target.as_str())
                .map_err(|e| anyhow!("invalid ref name {full_target}: {e}"))?,
            head_id,
            gix::refs::transaction::PreviousValue::MustExistAndMatch(
                gix::refs::Target::Object(prev_target),
            ),
            format!("merge: fast-forward {target_branch} to {head_sha}"),
        );
        repo.edit_references([edit])
            .map_err(|e| anyhow!("failed to update branch {target_branch}: {e}"))?;
        Ok(())
    }
}

fn git_cmd() -> std::process::Command {
    std::process::Command::new("git")
}

fn resolve_reference_id(repo: &Repository, reference: &str) -> Result<gix::ObjectId> {
    let full = if reference == "HEAD" || reference.contains("refs/") {
        reference.to_string()
    } else {
        format!("refs/heads/{reference}")
    };
    let reference = repo
        .find_reference(&full)
        .with_context(|| format!("find reference {full}"))?;
    Ok(reference.id().detach())
}

/// Like [`resolve_reference_id`], but returns `Ok(None)` when the reference
/// does not exist. This is used by read-only browsing operations so that an
/// empty repository (or a request for a not-yet-created branch) yields an
/// empty result instead of an error/500.
fn resolve_reference_id_opt(repo: &Repository, reference: &str) -> Result<Option<gix::ObjectId>> {
    let full = if reference == "HEAD" || reference.contains("refs/") {
        reference.to_string()
    } else {
        format!("refs/heads/{reference}")
    };
    match repo.find_reference(&full) {
        Ok(reference) => Ok(Some(reference.id().detach())),
        Err(_) => Ok(None),
    }
}

/// Recursively collect entries under `prefix_path`.
fn collect_tree_entries(
    tree: &gix::Tree<'_>,
    prefix_path: &str,
    out: &mut Vec<TreeEntry>,
) -> Result<()> {
    let base = if prefix_path.is_empty() {
        String::new()
    } else {
        format!("{prefix_path}/")
    };
    for entry in tree.iter() {
        let entry = entry?;
        let kind = if entry.mode().is_tree() { "tree" } else { "blob" };
        let path = format!("{}{}", base, entry.filename());
        out.push(TreeEntry {
            path: path.clone(),
            name: entry.filename().to_string(),
            kind: kind.to_string(),
            size: None,
        });
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
    bytes.windows(2).position(|w| w == b"\n\n").map(|pos| pos + 2)
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

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gita-git-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
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
                panic!("git command failed: {:?}\nstdout: {}\nstderr: {}", args, stdout, stderr);
            }
        };
        run(&["init", "-b", "main"]);
        std::fs::write(work.join("README.md"), "# Hello\n\nThis is a test repo.\n").unwrap();
        std::fs::write(work.join("main.rs"), "fn main() { println!(\"hi\"); }\n").unwrap();
        run(&["add", "."]);
        run(&["-c", "user.email=test@example.com", "-c", "user.name=Test", "commit", "-m", "initial commit"]);

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
        let repo_path = rt.block_on(async {
            mgr.create_repo("alice", "test", "main").await.unwrap()
        });
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
}