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

    /// The filesystem root where bare repositories live.
    pub fn root(&self) -> &std::path::Path {
        &self.root
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

    /// Move a bare repository from one namespace/name to another.
    pub fn rename_repo(
        &self,
        old_owner: &str,
        old_name: &str,
        new_owner: &str,
        new_name: &str,
    ) -> Result<()> {
        let old_path = self.repo_path(old_owner, old_name);
        let new_path = self.repo_path(new_owner, new_name);
        if !old_path.exists() {
            return Err(anyhow!("repository not found: {old_owner}/{old_name}"));
        }
        if new_path.exists() {
            return Err(anyhow!("target repository already exists: {new_owner}/{new_name}"));
        }
        if let Some(parent) = new_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&old_path, &new_path)?;
        Ok(())
    }

    /// Fork: clone the bare repository at `(src_owner, src_name)` to a
    /// new location under `(dst_owner, dst_name)`. The destination must
    /// not exist. The clone uses `git clone --bare` so all objects and
    /// refs are copied without a working tree.
    pub fn fork_repo(
        &self,
        src_owner: &str,
        src_name: &str,
        dst_owner: &str,
        dst_name: &str,
    ) -> Result<()> {
        let src = self.repo_path(src_owner, src_name);
        let dst = self.repo_path(dst_owner, dst_name);
        if !src.exists() {
            return Err(anyhow!("source repository not found: {src_owner}/{src_name}"));
        }
        if dst.exists() {
            return Err(anyhow!("destination repository already exists: {dst_owner}/{dst_name}"));
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let src_url = format!("file://{}", src.display());
        let output = git_cmd()
            .args(["clone", "--bare", "--no-local", &src_url])
            .arg(&dst)
            .output()
            .map_err(|e| anyhow!("fork git clone failed: {e}"))?;
        if !output.status.success() {
            return Err(anyhow!(
                "fork git clone error: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
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

    /// List tags of a repository.
    pub fn list_tags(&self, owner: &str, name: &str) -> Result<Vec<String>> {
        let repo = self.open(owner, name)?;
        let mut tags = Vec::new();
        for reference in repo.references()?.all()? {
            let name = reference?.name().to_string();
            if let Some(tag) = name.strip_prefix("refs/tags/") {
                tags.push(tag.to_string());
            }
        }
        tags.sort();
        Ok(tags)
    }

    /// Create a branch pointing at `start_sha` (or HEAD's commit).
    pub fn create_branch(
        &self,
        owner: &str,
        name: &str,
        branch: &str,
        start_sha: Option<&str>,
    ) -> Result<()> {
        let repo = self.open(owner, name)?;
        let full_name = gix::refs::FullName::try_from(format!("refs/heads/{branch}").as_str())
            .map_err(|e| anyhow!("invalid branch name {branch}: {e}"))?;
        if repo.find_reference(&full_name).is_ok() {
            return Err(anyhow!("branch already exists: {branch}"));
        }
        let target = match start_sha {
            Some(sha) => Some(
                gix::ObjectId::from_hex(gix::bstr::BStr::new(sha.as_bytes()))
                    .map_err(|e| anyhow!("invalid start sha {sha}: {e}"))?,
            ),
            None => None,
        };
        let target = match target {
            Some(t) => t,
            None => repo
                .head()?
                .into_peeled_id()
                .map_err(|e| anyhow!("cannot resolve HEAD: {e}"))?
                .detach(),
        };
        let edit = gix::refs::transaction::RefEdit::update(
            full_name,
            gix::refs::Target::Object(target),
            gix::refs::transaction::PreviousValue::MustNotExist,
            format!("branch: created {branch}"),
        );
        repo.edit_references([edit])
            .map_err(|e| anyhow!("failed to create branch {branch}: {e}"))?;
        Ok(())
    }

    /// Search every repository for `query` across the HEAD tree using
    /// `gix` (no shell-out, works on bare repositories).
    ///
    /// Each hit is returned as `(owner, repo, path, line_number, content)`.
    /// Repositories without a HEAD (empty, just initialised) are skipped.
    pub fn search_code(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(String, String, String, i64, String)>> {
        let mut out = Vec::new();
        if !self.root.exists() {
            return Ok(out);
        }
        let owner_entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(_) => return Ok(out),
        };
        for owner_entry in owner_entries.flatten() {
            let owner_ft = match owner_entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if !owner_ft.is_dir() {
                continue;
            }
            let owner = owner_entry.file_name().to_string_lossy().to_string();
            let repo_entries = match std::fs::read_dir(owner_entry.path()) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for repo_entry in repo_entries.flatten() {
                let repo_ft = match repo_entry.file_type() {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                if !repo_ft.is_dir() {
                    continue;
                }
                let repo_with_git = repo_entry.file_name().to_string_lossy().to_string();
                let repo_name = repo_with_git
                    .strip_suffix(".git")
                    .unwrap_or(&repo_with_git)
                    .to_string();
                let bare = repo_entry.path();
                // Skip empty repos (HEAD symbolic ref pointing to a non-existent branch).
                if bare.join("HEAD").exists() {
                    if let Ok(head_text) = std::fs::read_to_string(bare.join("HEAD")) {
                        if let Some(rest) = head_text.trim_start().strip_prefix("ref:") {
                            let head_ref = rest.trim();
                            if !bare.join(head_ref).exists() {
                                continue;
                            }
                        }
                    }
                }

                let Ok(repo_obj) = self.open(&owner, &repo_name) else {
                    continue;
                };
                let head_id = match repo_obj.head().and_then(|r| r.into_peeled_id()) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                let Ok(tree) = repo_obj.find_commit(head_id.detach()).and_then(|c| c.tree()) else {
                    continue;
                };
                Self::scan_tree_for_query(
                    &repo_obj,
                    &tree,
                    "",
                    query,
                    &owner,
                    &repo_name,
                    limit,
                    &mut out,
                );
            }
        }
        Ok(out)
    }

    /// Recursively walk a tree, scanning every blob's bytes for `query`.
    fn scan_tree_for_query(
        repo: &gix::Repository,
        tree: &gix::Tree<'_>,
        prefix: &str,
        query: &str,
        owner: &str,
        repo_name: &str,
        limit: usize,
        out: &mut Vec<(String, String, String, i64, String)>,
    ) {
        for entry in tree.iter() {
            if out.len() >= limit {
                return;
            }
            let Ok(entry) = entry else { continue };
            let filename = entry.filename().to_string();
            let path = if prefix.is_empty() {
                filename.clone()
            } else {
                format!("{prefix}/{filename}")
            };
            let Ok(entry_obj) = entry.object() else { continue };
            if entry_obj.kind == gix::object::Kind::Blob {
                let Ok(text) = std::str::from_utf8(&entry_obj.data) else {
                    continue;
                };
                for (idx, line) in text.lines().enumerate() {
                    if line.contains(query) {
                        out.push((
                            owner.to_string(),
                            repo_name.to_string(),
                            path.clone(),
                            (idx + 1) as i64,
                            line.to_string(),
                        ));
                        if out.len() >= limit {
                            return;
                        }
                    }
                }
            } else if entry_obj.kind == gix::object::Kind::Tree {
                let Ok(subtree) = repo.find_tree(entry_obj.id) else { continue };
                Self::scan_tree_for_query(
                    repo,
                    &subtree,
                    &path,
                    query,
                    owner,
                    repo_name,
                    limit,
                    out,
                );
            }
        }
    }
}

/// Delete a branch.
impl GitManager {
    pub fn delete_branch(&self, owner: &str, name: &str, branch: &str) -> Result<()> {
        let repo = self.open(owner, name)?;
        let full = gix::refs::FullName::try_from(format!("refs/heads/{branch}").as_str())
            .map_err(|e| anyhow!("invalid branch name {branch}: {e}"))?;
        let edit = gix::refs::transaction::RefEdit::delete(
            full,
            gix::refs::transaction::PreviousValue::Any,
        );
        repo.edit_references([edit])
            .map_err(|e| anyhow!("failed to delete branch {branch}: {e}"))?;
        Ok(())
    }

    /// Create a lightweight tag pointing at `start_sha` (or HEAD).
    pub fn create_tag(
        &self,
        owner: &str,
        name: &str,
        tag: &str,
        start_sha: Option<&str>,
    ) -> Result<()> {
        let repo = self.open(owner, name)?;
        let full_name = gix::refs::FullName::try_from(format!("refs/tags/{tag}").as_str())
            .map_err(|e| anyhow!("invalid tag name {tag}: {e}"))?;
        if repo.find_reference(&full_name).is_ok() {
            return Err(anyhow!("tag already exists: {tag}"));
        }
        let target = match start_sha {
            Some(sha) => Some(
                gix::ObjectId::from_hex(gix::bstr::BStr::new(sha.as_bytes()))
                    .map_err(|e| anyhow!("invalid start sha {sha}: {e}"))?,
            ),
            None => None,
        };
        let target = match target {
            Some(t) => t,
            None => repo
                .head()?
                .into_peeled_id()
                .map_err(|e| anyhow!("cannot resolve HEAD: {e}"))?
                .detach(),
        };
        let edit = gix::refs::transaction::RefEdit::update(
            full_name,
            gix::refs::Target::Object(target),
            gix::refs::transaction::PreviousValue::MustNotExist,
            format!("tag: created {tag}"),
        );
        repo.edit_references([edit])
            .map_err(|e| anyhow!("failed to create tag {tag}: {e}"))?;
        Ok(())
    }

    /// Delete a tag.
    pub fn delete_tag(&self, owner: &str, name: &str, tag: &str) -> Result<()> {
        let repo = self.open(owner, name)?;
        let full = gix::refs::FullName::try_from(format!("refs/tags/{tag}").as_str())
            .map_err(|e| anyhow!("invalid tag name {tag}: {e}"))?;
        let edit = gix::refs::transaction::RefEdit::delete(
            full,
            gix::refs::transaction::PreviousValue::Any,
        );
        repo.edit_references([edit])
            .map_err(|e| anyhow!("failed to delete tag {tag}: {e}"))?;
        Ok(())
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
        // Read HEAD directly via git CLI which always honours symbolic-ref.
        let path = self.repo_path(owner, name);
        let out = git_cmd()
            .arg("--git-dir")
            .arg(&path)
            .args(["symbolic-ref", "--short", "HEAD"])
            .output()
            .map_err(|e| anyhow!("run git symbolic-ref: {e}"))?;
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return Ok(s);
            }
        }
        // Fallback: see what HEAD points at and find the matching branch.
        let repo = self.open(owner, name)?;
        if let Ok(head) = repo.head() {
            if let Ok(head_id) = head.into_peeled_id() {
                let branch_names = repo.branch_names();
                for name in branch_names {
                    let Ok(full) = gix::refs::FullName::try_from(format!("refs/heads/{name}"))
                    else {
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
        // Peel the id to a commit first: a reference may legitimately point at
        // something that is not a commit, and a bare object id may not exist in
        // this repository at all. Both mean "no commits here" rather than an
        // error — `rev_walk` would otherwise fail hard on a missing object.
        let Ok(commit_id) = repo.find_commit(id).map(|c| c.id) else {
            return Ok(Vec::new());
        };
        let walk = repo
            .rev_walk([commit_id])
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

    /// Compute the diff between two refs (used for PR review).
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

    /// Run `git show <sha>` and return its combined patch (works for the
    /// root commit too, which has no parent).
    pub fn show_commit(
        &self,
        owner: &str,
        name: &str,
        sha: &str,
    ) -> Result<String> {
        let path = self.repo_path(owner, name);
        let output = git_cmd()
            .args(["--git-dir"])
            .arg(&path)
            .args(["show", "--no-color", "--no-ext-diff", sha])
            .output()
            .map_err(|e| anyhow!("git show failed: {e}"))?;
        if !output.status.success() {
            return Err(anyhow!(
                "git show error: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// List the files changed between `base_ref` and `head_ref` with their
    /// change status (A/M/D). Used by the PR detail page.
    pub fn diff_files_between(
        &self,
        owner: &str,
        name: &str,
        base_ref: &str,
        head_ref: &str,
    ) -> Result<Vec<(String, String)>> {
        let path = self.repo_path(owner, name);
        let output = git_cmd()
            .args(["--git-dir"])
            .arg(&path)
            .args(["diff", "--name-status", "--no-color", base_ref, head_ref])
            .output()
            .map_err(|e| anyhow!("git diff --name-status failed: {e}"))?;
        if !output.status.success() {
            return Err(anyhow!(
                "git diff --name-status error: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut out = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((status, path)) = line.split_once('\t') {
                out.push((status.to_string(), path.to_string()));
            } else {
                out.push((line.to_string(), String::new()));
            }
        }
        Ok(out)
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

    /// Fetch a single commit by SHA.
    pub fn get_commit(
        &self,
        owner: &str,
        name: &str,
        sha: &str,
    ) -> Result<gita_core::CommitMeta> {
        let repo = self.open(owner, name)?;
        let id = gix::ObjectId::from_hex(gix::bstr::BStr::new(sha.as_bytes()))
            .map_err(|e| anyhow!("invalid sha {sha}: {e}"))?;
        let commit = repo
            .find_commit(id)
            .map_err(|e| anyhow!("commit {sha} not found: {e}"))?;
        let parents: Vec<String> = commit
            .parent_ids()
            .map(|p| p.to_string())
            .collect();
        let sig = commit
            .author()
            .map_err(|e| anyhow!("author error: {e}"))?;
        let message = commit
            .message_raw()
            .map_err(|e| anyhow!("message error: {e}"))?;
        Ok(gita_core::CommitMeta {
            sha: commit.id.to_string(),
            author_name: sig.name.to_string(),
            author_email: sig.email.to_string(),
            message: message.to_string(),
            parent_shas: parents,
            authored_at: chrono::DateTime::from_timestamp(
                sig.time.parse().ok().unwrap_or(0),
                0,
            )
            .unwrap_or_else(chrono::Utc::now),
        })
    }

    /// Check whether `candidate` is an ancestor of `target` (i.e. updating
    /// `target` to `candidate` would be a fast-forward).
    pub fn is_ancestor(
        &self,
        owner: &str,
        name: &str,
        ancestor: &str,
        descendant: &str,
    ) -> Result<bool> {
        let path = self.repo_path(owner, name);
        let output = git_cmd()
            .args(["--git-dir"])
            .arg(&path)
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .output()
            .map_err(|e| anyhow!("git merge-base --is-ancestor failed: {e}"))?;
        Ok(output.status.success())
    }

    /// Detect conflicts when merging `head_sha` into `target_branch`.
    ///
    /// Uses a temporary index file so the bare repository is never
    /// mutated. Returns the list of conflicting paths (empty = clean merge).
    pub fn check_merge_conflict(
        &self,
        owner: &str,
        name: &str,
        target_branch: &str,
        head_sha: &str,
    ) -> Result<Vec<String>> {
        let repo = self.open(owner, name)?;
        let target_sha = resolve_reference_id(&repo, &format!("refs/heads/{target_branch}"))?
            .to_string();
        let path = self.repo_path(owner, name);

        let tmp = tempfile::tempdir().map_err(|e| anyhow!("create temp dir: {e}"))?;
        let index_file = tmp.path().join("index");
        let git_index = index_file.to_str().ok_or_else(|| anyhow!("bad temp path"))?;
        let work_tree = tmp.path().to_str().ok_or_else(|| anyhow!("bad temp path"))?;

        // Compute the merge base for a three-way merge.
        let merge_base = git_cmd()
            .args(["--git-dir"])
            .arg(&path)
            .args(["merge-base", &target_sha, head_sha])
            .output()
            .map_err(|e| anyhow!("git merge-base failed: {e}"))?;
        if !merge_base.status.success() {
            return Err(anyhow!(
                "git merge-base error: {}",
                String::from_utf8_lossy(&merge_base.stderr)
            ));
        }
        let base_sha = String::from_utf8_lossy(&merge_base.stdout).trim().to_string();
        if base_sha.is_empty() {
            return Err(anyhow!("no merge base between {target_sha} and {head_sha}"));
        }

        // Three-way merge: base + target + head, straight into an empty index.
        // (The temporary worktree starts empty, so no overwrite check fires.)
        let merge_tree = git_cmd()
            .env("GIT_INDEX_FILE", git_index)
            .env("GIT_WORK_TREE", work_tree)
            .args(["-c", "core.bare=false", "--git-dir"])
            .arg(&path)
            .args(["read-tree", "-m", &base_sha, &target_sha, head_sha])
            .output()
            .map_err(|e| anyhow!("git read-tree -m failed: {e}"))?;
        if !merge_tree.status.success() {
            return Err(anyhow!(
                "git read-tree merge error: {}",
                String::from_utf8_lossy(&merge_tree.stderr)
            ));
        }

        // `git write-tree` fails when the index has unmerged entries.
        let write_tree = git_cmd()
            .env("GIT_INDEX_FILE", git_index)
            .env("GIT_WORK_TREE", work_tree)
            .args(["-c", "core.bare=false", "--git-dir"])
            .arg(&path)
            .args(["write-tree"])
            .output()
            .map_err(|e| anyhow!("git write-tree failed: {e}"))?;
        if write_tree.status.success() {
            return Ok(Vec::new()); // clean merge
        }

        // Extract conflicting paths from `git ls-files --unmerged`.
        let ls_unmerged = git_cmd()
            .env("GIT_INDEX_FILE", git_index)
            .env("GIT_WORK_TREE", work_tree)
            .args(["-c", "core.bare=false", "--git-dir"])
            .arg(&path)
            .args(["ls-files", "--unmerged"])
            .output()
            .map_err(|e| anyhow!("git ls-files --unmerged failed: {e}"))?;
        let text = String::from_utf8_lossy(&ls_unmerged.stdout);
        let mut paths: Vec<String> = text
            .lines()
            .filter_map(|l| {
                // Format: "<mode> <sha> <stage>\t<path>"
                l.split_once('\t').map(|(_, p)| p.trim().to_string())
            })
            .collect();
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    /// Merge a source commit into a target branch.
    ///
    /// If `head_sha` is a descendant of the target (fast-forward), the
    /// target reference is simply advanced. Otherwise a merge commit is
    /// created with parents `(target_sha, head_sha)` via `git commit-tree`.
    /// When the merge has conflicts, returns an error listing the
    /// conflicting paths and leaves the repository untouched.
    pub fn merge_branch(
        &self,
        owner: &str,
        name: &str,
        target_branch: &str,
        head_sha: &str,
        author_name: &str,
        author_email: &str,
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

        let target_sha = target.id().to_string();
        let path = self.repo_path(owner, name);

        // Fast-forward path: head is a descendant of target.
        if self.is_ancestor(owner, name, &target_sha, head_sha)? {
            let edit = gix::refs::transaction::RefEdit::update(
                gix::refs::FullName::try_from(full_target.as_str())
                    .map_err(|e| anyhow!("invalid ref name {full_target}: {e}"))?,
                head_id,
                gix::refs::transaction::PreviousValue::MustExistAndMatch(
                    gix::refs::Target::Object(target.id().detach()),
                ),
                format!("merge: fast-forward {target_branch} to {head_sha}"),
            );
            repo.edit_references([edit])
                .map_err(|e| anyhow!("failed to update branch {target_branch}: {e}"))?;
            return Ok(());
        }

        // Non-fast-forward: detect conflicts first.
        let conflicts = self.check_merge_conflict(owner, name, target_branch, head_sha)?;
        if !conflicts.is_empty() {
            return Err(anyhow!(
                "merge conflict in: {}",
                conflicts.join(", ")
            ));
        }

        // Create a merge commit with parents (target, head).
        let message = format!(
            "Merge branch '{}' into {}\n\n(merged via Gita PR)",
            target_branch, head_sha
        );
        let merge_base = git_cmd()
            .args(["--git-dir"])
            .arg(&path)
            .args(["merge-base", &target_sha, head_sha])
            .output()
            .map_err(|e| anyhow!("git merge-base failed: {e}"))?;
        let base_sha = String::from_utf8_lossy(&merge_base.stdout).trim().to_string();
        if base_sha.is_empty() {
            return Err(anyhow!("no merge base between {target_sha} and {head_sha}"));
        }

        // The tree for the merge commit is the head commit's tree (a clean
        // merge with no conflicts means the head tree already contains all
        // changes; git's default strategy would normally re-derive it).
        let head_tree = git_cmd()
            .args(["--git-dir"])
            .arg(&path)
            .args(["rev-parse", &format!("{head_sha}^{{tree}}")])
            .output()
            .map_err(|e| anyhow!("git rev-parse tree failed: {e}"))?;
        let tree_sha = String::from_utf8_lossy(&head_tree.stdout).trim().to_string();
        if tree_sha.is_empty() {
            return Err(anyhow!("git rev-parse returned an empty tree SHA"));
        }

        let commit_tree = git_cmd()
            .args(["-c", &format!("user.name={author_name}"), "-c", &format!("user.email={author_email}")])
            .args(["--git-dir"])
            .arg(&path)
            .args([
                "commit-tree",
                &tree_sha,
                "-p",
                &target_sha,
                "-p",
                head_sha,
                "-m",
                &message,
            ])
            .output()
            .map_err(|e| anyhow!("git commit-tree failed: {e}"))?;
        if !commit_tree.status.success() {
            return Err(anyhow!(
                "git commit-tree error: {}",
                String::from_utf8_lossy(&commit_tree.stderr)
            ));
        }
        let merge_sha = String::from_utf8_lossy(&commit_tree.stdout).trim().to_string();
        if merge_sha.is_empty() {
            return Err(anyhow!("git commit-tree returned an empty SHA"));
        }
        let merge_id = gix::ObjectId::from_hex(gix::bstr::BStr::new(merge_sha.as_bytes()))
            .map_err(|e| anyhow!("invalid merge sha {merge_sha}: {e}"))?;

        let edit = gix::refs::transaction::RefEdit::update(
            gix::refs::FullName::try_from(full_target.as_str())
                .map_err(|e| anyhow!("invalid ref name {full_target}: {e}"))?,
            merge_id,
            gix::refs::transaction::PreviousValue::MustExistAndMatch(
                gix::refs::Target::Object(target.id().detach()),
            ),
            format!("merge: {target_branch} <- {head_sha}"),
        );
        repo.edit_references([edit])
            .map_err(|e| anyhow!("failed to update branch {target_branch}: {e}"))?;
        Ok(())
    }
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

fn resolve_reference_id(repo: &Repository, reference: &str) -> Result<gix::ObjectId> {
    match classify_reference(reference) {
        RefSpec::ObjectId(sha) => {
            gix::ObjectId::from_hex(gix::bstr::BStr::new(sha.as_bytes()))
                .map_err(|e| anyhow!("invalid sha {sha}: {e}"))
        }
        RefSpec::Ref(full) => {
            let reference = repo
                .find_reference(&full)
                .with_context(|| format!("find reference {full}"))?;
            Ok(reference.id().detach())
        }
    }
}

/// Like [`resolve_reference_id`], but returns `Ok(None)` when the reference
/// does not exist. This is used by read-only browsing operations so that an
/// empty repository (or a request for a not-yet-created branch) yields an
/// empty result instead of an error/500.
fn resolve_reference_id_opt(repo: &Repository, reference: &str) -> Result<Option<gix::ObjectId>> {
    match classify_reference(reference) {
        RefSpec::ObjectId(sha) => match gix::ObjectId::from_hex(gix::bstr::BStr::new(sha.as_bytes())) {
            Ok(id) => Ok(Some(id)),
            Err(_) => Ok(None),
        },
        RefSpec::Ref(full) => match repo.find_reference(&full) {
            Ok(reference) => Ok(Some(reference.id().detach())),
            Err(_) => Ok(None),
        },
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

    #[test]
    fn reference_accepts_full_commit_sha() {
        if !git_available() {
            eprintln!("git not available, skipping");
            return;
        }
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo_path = rt.block_on(async {
            mgr.create_repo("alice", "sha", "main").await.unwrap()
        });
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
        let repo_path = rt.block_on(async {
            mgr.create_repo("alice", "badsha", "main").await.unwrap()
        });
        push_initial_commit(&mgr, "alice", "badsha", &repo_path);

        // 40 chars but not hex: must degrade to an empty result, not panic or error.
        let bogus = "z".repeat(40);
        assert!(
            mgr.list_commits("alice", "badsha", &bogus, 10).unwrap().is_empty(),
            "malformed sha should yield no commits"
        );
        assert!(
            mgr.read_file("alice", "badsha", &bogus, "README.md").unwrap().is_none(),
            "read_file at a malformed sha should return None"
        );

        // A well-formed SHA that simply doesn't exist in this repo.
        let absent = "0".repeat(40);
        assert!(mgr.list_commits("alice", "badsha", &absent, 10).unwrap().is_empty());

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

    #[tokio::test]
    async fn create_repo_then_repo_exists() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        assert!(!mgr.repo_exists("alice", "test"), "fresh repo should not exist");
        mgr.create_repo("alice", "test", "main").await.unwrap();
        assert!(mgr.repo_exists("alice", "test"));
    }

    #[tokio::test]
    async fn create_repo_duplicate_is_an_error() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        mgr.create_repo("alice", "test", "main").await.unwrap();
        let err = mgr
            .create_repo("alice", "test", "main")
            .await
            .unwrap_err();
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
        assert!(res.is_none(), "missing file should return Ok(None), got {res:?}");
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

    #[tokio::test]
    async fn branch_and_tag_crud() {
        let root = temp_dir();
        let mgr = GitManager::new(root.clone());
        let path = mgr.create_repo("alice", "branchtag", "main").await.unwrap();
        push_initial_commit(&mgr, "alice", "branchtag", &path);

        // Create a branch from HEAD.
        mgr.create_branch("alice", "branchtag", "feature", None).unwrap();
        let branches = mgr.list_branches("alice", "branchtag").unwrap();
        assert!(branches.contains(&"feature".to_string()));

        // Create a branch from a specific sha (main head).
        let main_sha = mgr.head_sha("alice", "branchtag", "main").unwrap();
        mgr.create_branch("alice", "branchtag", "from-sha", Some(&main_sha)).unwrap();
        let branches = mgr.list_branches("alice", "branchtag").unwrap();
        assert!(branches.contains(&"from-sha".to_string()));

        // Duplicate branch -> error.
        let err = mgr.create_branch("alice", "branchtag", "feature", None).unwrap_err();
        assert!(err.to_string().contains("already exists"));

        // Delete branch.
        mgr.delete_branch("alice", "branchtag", "feature").unwrap();
        let branches = mgr.list_branches("alice", "branchtag").unwrap();
        assert!(!branches.contains(&"feature".to_string()));

        // Tags.
        mgr.create_tag("alice", "branchtag", "v1.0.0", Some(&main_sha)).unwrap();
        let tags = mgr.list_tags("alice", "branchtag").unwrap();
        assert!(tags.contains(&"v1.0.0".to_string()));

        // Duplicate tag -> error.
        let err = mgr.create_tag("alice", "branchtag", "v1.0.0", None).unwrap_err();
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
        mgr.rename_repo("alice", "before", "alice", "after").unwrap();
        assert!(!mgr.repo_exists("alice", "before"));
        assert!(mgr.repo_exists("alice", "after"));

        // Rename to a different namespace.
        mgr.rename_repo("alice", "after", "bob", "shared").unwrap();
        assert!(!mgr.repo_exists("alice", "after"));
        assert!(mgr.repo_exists("bob", "shared"));

        // Rename to existing target -> error.
        mgr.create_repo("alice", "collision", "main").await.unwrap();
        let err = mgr.rename_repo("bob", "shared", "alice", "collision").unwrap_err();
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
                panic!("git {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr));
            }
        };
        let bare_url = format!("file://{}", path.display());
        // Clone the existing branch into a fresh worktree.
        run(&["clone", "-q", "-b", "main", &bare_url, "."]);
        for (file, content) in files {
            std::fs::write(work.join(file), content).unwrap();
        }
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
        push_branch(&mgr, "alice", "ff", "feature", &[("feature.txt", "x\n")], "feature work");
        let main_before = mgr.head_sha("alice", "ff", "main").unwrap();
        let feature_sha = mgr.head_sha("alice", "ff", "feature").unwrap();

        // Fast-forward: feature is a descendant of main.
        assert!(mgr.is_ancestor("alice", "ff", &main_before, &feature_sha).unwrap());

        // Merge should advance main to feature sha.
        mgr.merge_branch("alice", "ff", "main", &feature_sha, "Test", "test@example.com").unwrap();
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
        push_branch(&mgr, "alice", "merge", "feature", &[("feat.md", "feature\n")], "feat");
        let feature_sha = mgr.head_sha("alice", "merge", "feature").unwrap();

        // Advance main independently (different file -> no conflict).
        push_branch(&mgr, "alice", "merge", "main", &[("main.md", "main\n")], "main work");
        let main_diverged = mgr.head_sha("alice", "merge", "main").unwrap();
        assert_ne!(main_diverged, feature_sha);

        // No conflict expected.
        let conflicts = mgr.check_merge_conflict("alice", "merge", "main", &feature_sha).unwrap();
        assert!(conflicts.is_empty(), "expected no conflict, got {conflicts:?}");

        // Merge should create a merge commit (not fast-forward).
        mgr.merge_branch("alice", "merge", "main", &feature_sha, "Test", "test@example.com").unwrap();
        let main_after = mgr.head_sha("alice", "merge", "main").unwrap();
        assert_ne!(main_after, feature_sha, "merge commit should not equal feature head");
        assert_ne!(main_after, main_diverged, "main should advance");
        // The new head should have two parents (merge commit).
        let parents = mgr
            .open("alice", "merge")
            .unwrap()
            .find_commit(gix::ObjectId::from_hex(gix::bstr::BStr::new(main_after.as_bytes())).unwrap())
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
        push_branch(&mgr, "alice", "conflict", "feature", &[("README.md", "# Feature\n")], "feat readme");
        let feature_sha = mgr.head_sha("alice", "conflict", "feature").unwrap();

        // Main also modifies README.md differently.
        push_branch(&mgr, "alice", "conflict", "main", &[("README.md", "# Main\n")], "main readme");
        let main_sha = mgr.head_sha("alice", "conflict", "main").unwrap();
        assert!(!mgr.is_ancestor("alice", "conflict", &main_sha, &feature_sha).unwrap());

        // Conflict detection should report README.md.
        let conflicts = mgr.check_merge_conflict("alice", "conflict", "main", &feature_sha).unwrap();
        assert!(
            conflicts.iter().any(|p| p == "README.md"),
            "expected README.md conflict, got {conflicts:?}"
        );

        // merge_branch must fail with a conflict error and leave main untouched.
        let err = mgr.merge_branch("alice", "conflict", "main", &feature_sha, "Test", "test@example.com").unwrap_err();
        assert!(err.to_string().contains("merge conflict"), "got: {err}");
        let main_after = mgr.head_sha("alice", "conflict", "main").unwrap();
        assert_eq!(main_after, main_sha, "main must not change on conflict");

        let _ = std::fs::remove_dir_all(&root);
    }
}