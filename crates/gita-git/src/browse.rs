//! browse.rs
use anyhow::{anyhow, Result};
use gita_core::{Commit, FileContent, TreeEntry};
use crate::FileBlob;
use std::collections::HashSet;

use crate::GitManager;
use crate::{collect_tree_entries, find_entry_by_path, resolve_reference_id, resolve_reference_id_opt};
    /// Search every repository for `query` across the HEAD tree using
    /// `gix` (no shell-out, works on bare repositories).
    ///
    /// Each hit is returned as `(owner, repo, path, line_number, content)`.
    /// Repositories without a HEAD (empty, just initialised) are skipped.
    ///
    /// `allowed` restricts which repositories are scanned. The caller
    /// supplies the set of repositories the requesting user is allowed to
    /// read; a repository outside that set is **not** scanned at all, so its
    /// contents cannot leak through the results. `None` means "no
    /// restriction" and is only appropriate for callers that have already
    /// established that every repository is visible.
impl GitManager {
    pub fn search_code(
        &self,
        query: &str,
        limit: usize,
        allowed: Option<&HashSet<(String, String)>>,
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
                // Skip repositories the caller is not allowed to read.
                if let Some(allowed) = allowed {
                    if !allowed.contains(&(owner.clone(), repo_name.clone())) {
                        continue;
                    }
                }
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
                let Ok(tree) = repo_obj
                    .find_commit(head_id.detach())
                    .and_then(|c| c.tree())
                else {
                    continue;
                };
                Self::scan_tree_for_query(
                    &repo_obj, &tree, "", query, &owner, &repo_name, limit, &mut out,
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
            let Ok(entry_obj) = entry.object() else {
                continue;
            };
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
                let Ok(subtree) = repo.find_tree(entry_obj.id) else {
                    continue;
                };
                Self::scan_tree_for_query(
                    repo, &subtree, &path, query, owner, repo_name, limit, out,
                );
            }
        }
    }

    /// List files at and below a directory of a repository at a given ref.
    ///
    /// The listing is recursive: `path = "docs"` returns every entry under
    /// `docs/`, not just its immediate children.
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
        let commit_tree = repo.find_commit(id)?.tree()?;
        let mut entries = Vec::new();
        // Scope to the requested subtree first. An empty path means the whole
        // repository; a path that doesn't exist yields an empty listing rather
        // than falling back to the root.
        let scope = path.trim_matches('/');
        let tree = if scope.is_empty() {
            commit_tree
        } else {
            match commit_tree.lookup_entry_by_path(scope) {
                Ok(Some(entry)) if entry.mode().is_tree() => {
                    repo.find_tree(entry.oid().to_owned())?
                }
                // Not a directory: an empty listing, same as a missing path.
                _ => return Ok(Vec::new()),
            }
        };
        collect_tree_entries(&repo, &tree, scope, &mut entries)?;
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
        // A directory resolves to a tree entry; there is no text content to
        // return for it, so treat it as "not a file".
        if !entry.mode().is_blob() {
            return Ok(None);
        }
        let object = entry.object()?.into_blob();
        let content = String::from_utf8_lossy(&object.data).to_string();
        Ok(Some(FileContent {
            path: path.to_string(),
            content,
            sha: entry.oid().to_string(),
        }))
    }

    /// Read a file's raw bytes at a given ref and path.
    ///
    /// Unlike [`GitManager::read_file`], this never decodes the blob, so it is
    /// lossless for binary content (images, fonts, wasm). Use it whenever the
    /// bytes are re-emitted rather than displayed; `read_file` remains the
    /// right choice for rendering source text, where lossy UTF-8 is a
    /// tolerable (and currently relied-upon) simplification.
    pub fn read_file_bytes(
        &self,
        owner: &str,
        name: &str,
        reference: &str,
        path: &str,
    ) -> Result<Option<FileBlob>> {
        let repo = self.open(owner, name)?;
        let Some(id) = resolve_reference_id_opt(&repo, reference)? else {
            return Ok(None);
        };
        let commit = repo.find_commit(id)?;
        let tree = commit.tree()?;
        let entry = match find_entry_by_path(&tree, path) {
            Ok(e) => e,
            Err(_) => return Ok(None),
        };
        // Directories and submodules are not servable file content.
        if !entry.mode().is_blob() {
            return Ok(None);
        }
        let object = entry.object()?.into_blob();
        // `object` borrows from the repository, so copy the bytes out while it
        // is still in scope rather than trying to move them.
        let data = object.data.to_vec();
        Ok(Some(FileBlob {
            path: path.to_string(),
            size: data.len() as u64,
            data,
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
                authored_at: chrono::DateTime::from_timestamp(committer.seconds(), 0)
                    .unwrap_or_else(chrono::Utc::now),
            });
        }
        Ok(commits)
    }

    /// Fetch a single commit by reference.
    ///
    /// The reference may be a full object id or any ref name — `HEAD`,
    /// a short branch name, or a fully-qualified `refs/...`. It is
    /// resolved through the same `classify_reference` /
    /// `resolve_reference_id` path the tree and file readers use, so
    /// `get_commit("HEAD")` and `list_tree("HEAD")` agree.
    pub fn get_commit(&self, owner: &str, name: &str, sha: &str) -> Result<gita_core::CommitMeta> {
        let repo = self.open(owner, name)?;
        let id = resolve_reference_id(&repo, sha)?;
        let commit = repo
            .find_commit(id)
            .map_err(|e| anyhow!("commit {sha} not found: {e}"))?;
        let parents: Vec<String> = commit.parent_ids().map(|p| p.to_string()).collect();
        let sig = commit.author().map_err(|e| anyhow!("author error: {e}"))?;
        let message = commit
            .message_raw()
            .map_err(|e| anyhow!("message error: {e}"))?;
        Ok(gita_core::CommitMeta {
            sha: commit.id.to_string(),
            author_name: sig.name.to_string(),
            author_email: sig.email.to_string(),
            message: message.to_string(),
            parent_shas: parents,
            authored_at: chrono::DateTime::from_timestamp(sig.time.parse().ok().unwrap_or(0), 0)
                .unwrap_or_else(chrono::Utc::now),
        })
    }
}
