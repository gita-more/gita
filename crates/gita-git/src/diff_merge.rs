//! diff_merge.rs
use anyhow::{anyhow, Context, Result};
use gix;

use crate::GitManager;
use crate::{git_cmd, resolve_reference_id};
    /// Compute the diff between two refs (used for PR review).
impl GitManager {
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
            .args([
                "diff",
                "--no-color",
                "--no-ext-diff",
                base_ref,
                head_ref,
                "--",
            ])
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
    pub fn show_commit(&self, owner: &str, name: &str, sha: &str) -> Result<String> {
        let path = self.repo_path(owner, name);
        let output = git_cmd()
            .args(["--git-dir"])
            .arg(&path)
            // `--end-of-options` tells git that everything after it is a
            // revision, not an option. Without it a reference such as
            // `--output=/tmp/x` would be taken as an option.
            .args([
                "show",
                "--no-color",
                "--no-ext-diff",
                "--end-of-options",
                sha,
            ])
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
            .args([
                "diff",
                "--name-status",
                "--no-color",
                "--end-of-options",
                base_ref,
                head_ref,
            ])
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
            .args([
                "rev-list",
                "--reverse",
                "--end-of-options",
                &format!("{base_ref}..{head_ref}"),
            ])
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
            .args([
                "merge-base",
                "--is-ancestor",
                "--end-of-options",
                ancestor,
                descendant,
            ])
            .output()
            .map_err(|e| anyhow!("git merge-base --is-ancestor failed: {e}"))?;
        Ok(output.status.success())
    }

    /// Detect conflicts when merging `head_sha` into `target_branch`.
    ///
    /// Uses a temporary index file so the bare repository is never
    /// mutated. Returns the list of conflicting paths (empty = clean merge).
    /// Compute the three-way merge of `head_sha` into `target_branch`.
    ///
    /// Returns the merge tree's object id on a clean merge, or the list of
    /// conflicting paths when the merge cannot be resolved automatically.
    ///
    /// The merge tree is the *correct* result of merging: it contains the
    /// target branch's changes **and** the source branch's changes. Using
    /// the source commit's tree instead would silently drop everything the
    /// target branch changed on its own.
    fn compute_merge_tree(
        &self,
        path: &std::path::Path,
        target_sha: &str,
        head_sha: &str,
    ) -> Result<Result<String, Vec<String>>> {
        let tmp = tempfile::tempdir().map_err(|e| anyhow!("create temp dir: {e}"))?;
        let index_file = tmp.path().join("index");
        let git_index = index_file
            .to_str()
            .ok_or_else(|| anyhow!("bad temp path"))?;
        let work_tree = tmp
            .path()
            .to_str()
            .ok_or_else(|| anyhow!("bad temp path"))?;

        // Compute the merge base for a three-way merge.
        let merge_base = git_cmd()
            .args(["--git-dir"])
            .arg(&path)
            .args(["merge-base", "--end-of-options", target_sha, head_sha])
            .output()
            .map_err(|e| anyhow!("git merge-base failed: {e}"))?;
        if !merge_base.status.success() {
            return Err(anyhow!(
                "git merge-base error: {}",
                String::from_utf8_lossy(&merge_base.stderr)
            ));
        }
        let base_sha = String::from_utf8_lossy(&merge_base.stdout)
            .trim()
            .to_string();
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
            .args([
                "read-tree",
                "-m",
                "--end-of-options",
                base_sha.as_str(),
                target_sha,
                head_sha,
            ])
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
            let tree = String::from_utf8_lossy(&write_tree.stdout)
                .trim()
                .to_string();
            return Ok(Ok(tree));
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
        Ok(Err(paths))
    }

    /// List the paths that conflict when merging `head_sha` into
    /// `target_branch`. Empty means the merge is clean.
    ///
    /// The repository is never mutated: the merge is computed in a
    /// temporary index.
    pub fn check_merge_conflict(
        &self,
        owner: &str,
        name: &str,
        target_branch: &str,
        head_sha: &str,
    ) -> Result<Vec<String>> {
        let repo = self.open(owner, name)?;
        let target_sha =
            resolve_reference_id(&repo, &format!("refs/heads/{target_branch}"))?.to_string();
        let path = self.repo_path(owner, name);

        match self.compute_merge_tree(&path, &target_sha, head_sha)? {
            Ok(_tree) => Ok(Vec::new()),
            Err(conflicts) => Ok(conflicts),
        }
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

        // Create a merge commit with parents (target, head).
        let message = format!(
            "Merge branch '{}' into {}\n\n(merged via Gita PR)",
            target_branch, head_sha
        );

        // The merge commit's tree is the *merge tree*: the three-way merge
        // of the target branch and the head commit. Using the head commit's
        // tree instead would silently drop every change the target branch
        // made on its own — "no conflicts" only means the two sides did not
        // touch the same lines, not that the head tree already contains the
        // target's work.
        //
        // `compute_merge_tree` also reports conflicts, so no separate
        // pre-check is needed.
        let tree_sha = match self.compute_merge_tree(&path, &target_sha, head_sha)? {
            Ok(tree) => tree,
            Err(conflicts) => {
                return Err(anyhow!("merge conflict in: {}", conflicts.join(", ")));
            }
        };
        if tree_sha.is_empty() {
            return Err(anyhow!("git write-tree returned an empty tree SHA"));
        }

        let commit_tree = git_cmd()
            .args([
                "-c",
                &format!("user.name={author_name}"),
                "-c",
                &format!("user.email={author_email}"),
            ])
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
        let merge_sha = String::from_utf8_lossy(&commit_tree.stdout)
            .trim()
            .to_string();
        if merge_sha.is_empty() {
            return Err(anyhow!("git commit-tree returned an empty SHA"));
        }
        let merge_id = gix::ObjectId::from_hex(gix::bstr::BStr::new(merge_sha.as_bytes()))
            .map_err(|e| anyhow!("invalid merge sha {merge_sha}: {e}"))?;

        let edit = gix::refs::transaction::RefEdit::update(
            gix::refs::FullName::try_from(full_target.as_str())
                .map_err(|e| anyhow!("invalid ref name {full_target}: {e}"))?,
            merge_id,
            gix::refs::transaction::PreviousValue::MustExistAndMatch(gix::refs::Target::Object(
                target.id().detach(),
            )),
            format!("merge: {target_branch} <- {head_sha}"),
        );
        repo.edit_references([edit])
            .map_err(|e| anyhow!("failed to update branch {target_branch}: {e}"))?;
        Ok(())
    }
}
