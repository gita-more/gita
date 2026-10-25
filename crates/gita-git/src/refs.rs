//! refs.rs
use anyhow::{anyhow, Result};
use gix;

use crate::GitManager;
use crate::{git_cmd, resolve_reference_id, resolve_reference_id_opt};
    /// List refs (branches) of a repository.
impl GitManager {
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
        // Peel rather than calling `id()` directly: `id()` panics on a symbolic
        // reference such as HEAD.
        let id = resolve_reference_id(&repo, branch)?;
        Ok(id.to_string())
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
            Ok(branches
                .first()
                .cloned()
                .unwrap_or_else(|| "main".to_string()))
        }
    }

    /// Whether `reference` resolves to an existing commit in the repository.
    pub fn ref_exists(&self, owner: &str, name: &str, reference: &str) -> bool {
        let Ok(repo) = self.open(owner, name) else {
            return false;
        };
        let Ok(Some(id)) = resolve_reference_id_opt(&repo, reference) else {
            return false;
        };
        // Bind the result before returning: `find_commit` yields a value that
        // borrows `repo`, and a trailing expression would drop it too late.
        let exists = repo.find_commit(id).is_ok();
        exists
    }

    /// Get the SHA of the latest commit on a branch.
    pub fn head_sha(&self, owner: &str, name: &str, reference: &str) -> Result<String> {
        let repo = self.open(owner, name)?;
        let id = resolve_reference_id(&repo, reference)?;
        Ok(id.to_string())
    }
}
