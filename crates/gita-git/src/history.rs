//! history.rs
//!
//! Per-path commit attribution for the directory browser: the newest commit
//! that touched each entry, computed by walking history once and diffing
//! successive commits against their first parent (the same accounting GitHub
//! uses for its "last commit message / date" columns).
use anyhow::Result;
use gita_core::EntryCommit;
use gix::bstr::ByteSlice;
use gix::Repository;
use std::collections::HashMap;

use crate::GitManager;
use crate::resolve_reference_id_opt;

/// Upper bound on how many commits `latest_commits_for_paths` will walk.
/// Anything not attributed by then stays without a last commit — the UI
/// renders an em dash, which beats an unbounded walk on huge histories.
const MAX_HISTORY_WALK: usize = 500;

impl GitManager {
    /// List the immediate children of a directory (non-recursive), each
    /// decorated with the newest commit that touched it.
    ///
    /// Unlike [`GitManager::list_tree`], this is a single-level listing —
    /// what the web file browser shows for one directory, GitHub-style.
    pub fn list_dir(
        &self,
        owner: &str,
        name: &str,
        reference: &str,
        path: &str,
    ) -> Result<Vec<gita_core::TreeEntry>> {
        let repo = self.open(owner, name)?;
        let Some(id) = resolve_reference_id_opt(&repo, reference)? else {
            return Ok(Vec::new());
        };
        let commit_tree = repo.find_commit(id)?.tree()?;
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

        let mut entries = Vec::new();
        for entry in tree.iter() {
            let entry = entry?;
            let is_tree = entry.mode().is_tree();
            let full_path = if scope.is_empty() {
                entry.filename().to_string()
            } else {
                format!("{scope}/{}", entry.filename())
            };
            entries.push(gita_core::TreeEntry {
                path: full_path,
                name: entry.filename().to_string(),
                kind: if is_tree { "tree" } else { "blob" }.to_string(),
                size: None,
                last_commit: None,
            });
        }

        let paths: Vec<String> = entries.iter().map(|e| e.path.clone()).collect();
        let commits = latest_commits_for_paths(&repo, id, &paths)?;
        for entry in entries.iter_mut() {
            if let Some(commit) = commits.get(&entry.path) {
                entry.last_commit = Some(commit.clone());
            }
        }
        Ok(entries)
    }
}

/// For each path in `paths`, find the newest commit reachable from `tip`
/// that touched it — exact match for blobs, any descendant path for trees.
///
/// History is walked newest-first and the walk stops as soon as every path
/// is attributed (or [`MAX_HISTORY_WALK`] commits have been examined).
fn latest_commits_for_paths(
    repo: &Repository,
    tip: gix::ObjectId,
    paths: &[String],
) -> Result<HashMap<String, EntryCommit>> {
    let mut out = HashMap::new();
    if paths.is_empty() {
        return Ok(out);
    }
    let mut remaining: Vec<String> = paths.to_vec();

    let walk = repo
        .rev_walk([tip])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .selected(|_| true)?
        .take(MAX_HISTORY_WALK);

    for info in walk {
        if remaining.is_empty() {
            break;
        }
        let commit = info?.object()?;
        let tree = commit.tree()?;
        // Diff against the first parent (or the empty tree for root commits),
        // mirroring how `git log --name-only` accounts changes.
        let parent_tree = match commit.parent_ids().next() {
            Some(pid) => Some(repo.find_commit(pid)?.tree()?),
            None => None,
        };
        let empty_tree = repo.empty_tree();
        let lhs = parent_tree.as_ref().unwrap_or(&empty_tree);
        let mut changed: Vec<Vec<u8>> = Vec::new();
        lhs.changes()?
            .for_each_to_obtain_tree(&tree, |change| {
                changed.push(change.location().to_vec());
                Ok(std::ops::ControlFlow::Continue(()))
            })?;

        let sha = commit.id.to_string();
        let message = first_line(commit.message_raw_sloppy().to_str().unwrap_or(""));
        let author_name = commit.author()?.name.to_string();
        let authored_at = {
            let secs = commit.committer()?.seconds();
            chrono::DateTime::from_timestamp(secs, 0).unwrap_or_else(chrono::Utc::now)
        };

        let mut attributed = Vec::new();
        for path in &remaining {
            let touched = changed.iter().any(|c| {
                c.as_slice() == path.as_bytes()
                    || c.starts_with(&[path.as_bytes(), b"/"].concat())
            });
            if touched {
                out.insert(
                    path.clone(),
                    EntryCommit {
                        sha: sha.clone(),
                        message: message.clone(),
                        author_name: author_name.clone(),
                        authored_at,
                    },
                );
                attributed.push(path.clone());
            }
        }
        remaining.retain(|p| !attributed.contains(p));
    }
    Ok(out)
}

/// First line of a commit message, trimmed.
fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or("").trim().to_string()
}
