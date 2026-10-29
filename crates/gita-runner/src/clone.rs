//! Repository cloning shared by the pipeline and workflow executors.
//!
//! Both execution paths need the same thing: a checkout of the repository
//! at the exact commit the run was created for. This module owns that
//! logic so the two executors cannot drift apart.
//!
//! Two properties matter and are easy to get wrong:
//!
//! * The run stores a **full ref** (`refs/heads/main`), but `git clone
//!   --branch` wants a short branch name — `refs/heads/main` fails with
//!   "Remote branch not found".
//! * The branch tip may have **moved on** since the run was queued. The
//!   run must execute the commit it names, not the current tip; running
//!   a different commit than the run reports is worse than failing.

use crate::config::RunnerConfig;
use crate::workspace::clone_url;
use anyhow::{Context, Result};
use gita_db::{Db, PipelineRunRow};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

/// The outcome of preparing the run's checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneOutcome {
    /// The repository is checked out at the run's commit.
    Ready(PathBuf),
    /// The repository could not be prepared; the run must be failed.
    Failed(String),
}

/// Clone (or reuse) the repository for `run` and check it out at
/// `run.head_sha`.
///
/// If the checkout directory already contains a `.git`, the clone is
/// skipped — this is what lets an integration test pre-clone the
/// repository. The checkout is *not* re-verified in that case.
pub async fn prepare_run_repo(
    db: &Db,
    config: &RunnerConfig,
    run: &PipelineRunRow,
) -> Result<CloneOutcome> {
    let work_dir = crate::workspace::run_work_dir(config, run.id);
    tokio::fs::create_dir_all(&work_dir)
        .await
        .with_context(|| format!("create work dir {}", work_dir.display()))?;

    // The run row stores ref + head_sha but not owner/name — resolve them
    // from the repository the run points at.
    let Some((owner, repo_name)) = resolve_repo_owner_name(db, run).await? else {
        // Repo was deleted while the run was queued.
        return Ok(CloneOutcome::Failed("repository no longer exists".into()));
    };

    let repo_dir = work_dir.join("repo");
    let url = clone_url(config, &owner, &repo_name);

    // Already present (a prior run left it, or a test pre-cloned it).
    if repo_dir.join(".git").exists() {
        return Ok(CloneOutcome::Ready(repo_dir));
    }

    // Clone by short branch name; `refs/heads/main` is not a branch name
    // git accepts here.
    let short_branch = run
        .ref_name
        .strip_prefix("refs/heads/")
        .unwrap_or(&run.ref_name);

    // Pipe the child's output rather than inheriting it: a clone that
    // fails should have its error surfaced, not swallowed into the
    // runner's own stdout.
    let clone_output = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--branch")
        .arg(short_branch)
        .arg(&url)
        .arg(&repo_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("git clone")?;

    if !clone_output.status.success() {
        let stderr = String::from_utf8_lossy(&clone_output.stderr);
        return Ok(CloneOutcome::Failed(format!(
            "git clone failed: {}",
            stderr.trim()
        )));
    }

    if run.head_sha.is_empty() {
        return Ok(CloneOutcome::Ready(repo_dir));
    }

    // Check out the exact commit the run was created for. The clone
    // fetched the branch tip, which may have moved on since the run was
    // queued — the run would then execute a different commit than the
    // one it reports. A shallow clone may not contain the SHA at all, so
    // fetch it explicitly first.
    let fetch_status = Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("fetch")
        .arg("--depth")
        .arg("1")
        .arg("origin")
        .arg(&run.head_sha)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .await
        .context("git fetch")?;

    let have_commit = Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("cat-file")
        .arg("-e")
        .arg(format!("{}^{{commit}}", run.head_sha))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if !fetch_status.success() && !have_commit {
        // The commit is no longer reachable (force-pushed away). Falling
        // back to the branch tip would make the run report `head_sha`
        // while executing something else.
        return Ok(CloneOutcome::Failed(format!(
            "commit {} is no longer reachable",
            run.head_sha
        )));
    }

    let checkout = Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .arg("checkout")
        .arg("--detach")
        .arg(&run.head_sha)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("git checkout")?;

    if !checkout.status.success() {
        let stderr = String::from_utf8_lossy(&checkout.stderr);
        return Ok(CloneOutcome::Failed(format!(
            "checkout of {} failed: {}",
            run.head_sha,
            stderr.trim()
        )));
    }

    Ok(CloneOutcome::Ready(repo_dir))
}

/// Resolve `(owner, repo_name)` for a run from the repositories table.
pub async fn resolve_repo_owner_name(
    db: &Db,
    run: &PipelineRunRow,
) -> Result<Option<(String, String)>> {
    let repo = db.get_repository(run.repo_id).await?;
    Ok(repo.map(|r| (r.owner_name, r.name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_outcome_carries_a_reason() {
        let outcome = CloneOutcome::Failed("boom".into());
        assert_eq!(outcome, CloneOutcome::Failed("boom".into()));
    }

    #[test]
    fn a_ready_outcome_carries_the_repo_dir() {
        let p = PathBuf::from("/tmp/x");
        assert_eq!(CloneOutcome::Ready(p.clone()), CloneOutcome::Ready(p));
    }
}
