//! Parsing and validation of the ref updates a git push claims.

use gita_git::GitManager;

/// A ref update that was applied to a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefUpdate {
    /// The sha the ref pointed at before, or all zeroes for a creation.
    pub old_sha: String,
    /// The sha the ref points at now, or all zeroes for a deletion.
    pub new_sha: String,
    /// The full ref name, e.g. `refs/heads/main`.
    pub ref_name: String,
}

impl RefUpdate {
    /// Whether this update deletes the ref.
    pub fn is_deletion(&self) -> bool {
        self.new_sha.chars().all(|c| c == '0') && !self.new_sha.is_empty()
    }
}

/// Parse the ref-update commands a client sent to `git receive-pack`.
///
/// The commands are a sequence of pkt-lines (a four-byte ASCII-hex length
/// that includes the length bytes, then the payload), each of the form
///
/// ```text
/// <old-sha> SP <new-sha> SP <ref-name> NUL <capabilities> LF
/// ```
///
/// and terminated by a flush packet. The capabilities section only
/// appears on the first command.
///
/// This is the *client's* claim about what it is pushing, not a record of
/// what happened: the server may refuse. Use [`ref_update_applied`] to
/// find out which of the claims actually took effect.
pub fn parse_receive_pack_commands(body: &[u8]) -> Vec<RefUpdate> {
    let mut out = Vec::new();
    if body.is_empty() {
        return out;
    }

    // Skip the initial flush packet (`0000`) if present.
    let mut pos = 0usize;
    if body.len() >= 4 && &body[..4] == b"0000" {
        pos = 4;
    }

    while pos + 4 <= body.len() {
        let len_hex = &body[pos..pos + 4];
        let Ok(len_str) = std::str::from_utf8(len_hex) else {
            break;
        };
        let Ok(len) = usize::from_str_radix(len_str, 16) else {
            break;
        };
        if len == 0 {
            // Flush packet — end of the command section.
            break;
        }
        if len < 4 || pos + len > body.len() {
            break;
        }
        let line = &body[pos + 4..pos + len];
        pos += len;

        // Trim trailing LF.
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        let line = line.strip_suffix(b"\r").unwrap_or(line);

        // First line may have a NUL-separated capabilities section.
        let (ref_part, _caps) = match line.iter().position(|&b| b == 0) {
            Some(idx) => (&line[..idx], &line[idx + 1..]),
            None => (line, &[][..]),
        };

        let fields: Vec<&[u8]> = ref_part.split(|&b| b == b' ').collect();
        if fields.len() < 3 {
            continue;
        }
        out.push(RefUpdate {
            old_sha: String::from_utf8_lossy(fields[0]).to_string(),
            new_sha: String::from_utf8_lossy(fields[1]).to_string(),
            ref_name: String::from_utf8_lossy(fields[2]).to_string(),
        });
    }

    out
}

/// Whether a claimed ref update actually took effect.
///
/// The client says which refs it is updating; the repository may refuse.
/// A claim counts as applied only if the ref now points at the new sha —
/// for a deletion, only if the ref is gone. Otherwise a rejected push
/// would publish an event for a commit that was never written and would
/// trigger CI for it.
pub fn ref_update_applied(git: &GitManager, owner: &str, repo_name: &str, upd: &RefUpdate) -> bool {
    // A branch deletion is reported by the ref simply being gone.
    if upd.is_deletion() {
        return !git.ref_exists(owner, repo_name, &upd.ref_name);
    }
    match git.resolve_branch(owner, repo_name, &upd.ref_name) {
        Ok(current) => current == upd.new_sha,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gita_git::GitManager;

    fn run_git(args: &[&str], cwd: &std::path::Path) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Commit one file to `branch` of the bare repository and return the
    /// sha the branch points at afterwards.
    fn commit_to(
        root: &std::path::Path,
        owner: &str,
        name: &str,
        branch: &str,
        file: &str,
        content: &[u8],
    ) -> String {
        let bare = root.join(owner).join(format!("{name}.git"));
        let url = format!("file://{}", bare.display());
        let work = tempfile::TempDir::new().expect("work tree");
        // A freshly created bare repository has no `main` yet, so the
        // clone fails; fall back to initializing on the target branch.
        let clone_ok = std::process::Command::new("git")
            .args(["clone", "-q", "-b", "main", &url, "."])
            .current_dir(work.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !clone_ok {
            run_git(&["init", "-q", "-b", "main"], work.path());
            run_git(&["remote", "add", "origin", &url], work.path());
        }
        let dest = work.path().join(file);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, content).unwrap();
        run_git(&["checkout", "-q", "-b", branch], work.path());
        run_git(&["add", "."], work.path());
        run_git(&["commit", "-q", "-m", "test commit"], work.path());
        run_git(&["push", "-q", "-u", "origin", branch], work.path());
        let out = std::process::Command::new("git")
            .args(["rev-parse", &format!("origin/{branch}")])
            .current_dir(work.path())
            .output()
            .expect("rev-parse");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A manager with one repository holding `main` and `side`.
    async fn repo() -> (tempfile::TempDir, GitManager, String, String) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let git = GitManager::new(dir.path().to_path_buf());
        git.create_repo("alice", "widgets", "main")
            .await
            .expect("create repo");
        let sha_main = commit_to(dir.path(), "alice", "widgets", "main", "main.txt", b"main\n");
        let sha_side = commit_to(dir.path(), "alice", "widgets", "side", "side.txt", b"side\n");
        (dir, git, sha_main, sha_side)
    }

    fn zeros() -> String {
        "0".repeat(40)
    }

    /// A claim counts as applied only if the ref now points at the sha
    /// the client claimed — otherwise a rejected push would trigger CI
    /// for a commit that was never written.
    #[tokio::test]
    async fn a_claim_that_landed_counts_as_applied() {
        let (_dir, git, sha_main, _sha_side) = repo().await;
        let upd = RefUpdate {
            old_sha: zeros(),
            new_sha: sha_main.clone(),
            ref_name: "refs/heads/main".to_string(),
        };
        assert!(ref_update_applied(&git, "alice", "widgets", &upd));
    }

    #[tokio::test]
    async fn a_claim_that_did_not_land_is_refused() {
        let (_dir, git, _sha_main, sha_side) = repo().await;

        // `main` points elsewhere than the claim says.
        let upd = RefUpdate {
            old_sha: zeros(),
            new_sha: sha_side.clone(),
            ref_name: "refs/heads/main".to_string(),
        };
        assert!(!ref_update_applied(&git, "alice", "widgets", &upd));

        // The claimed ref does not exist at all.
        let upd = RefUpdate {
            old_sha: zeros(),
            new_sha: sha_side,
            ref_name: "refs/heads/ghost".to_string(),
        };
        assert!(!ref_update_applied(&git, "alice", "widgets", &upd));
    }

    /// A deletion is only "applied" once the ref is actually gone.
    #[tokio::test]
    async fn a_deletion_claim_counts_as_applied_only_when_the_ref_is_gone() {
        let (dir, git, _sha_main, _sha_side) = repo().await;
        let deletion = RefUpdate {
            old_sha: "0".to_string(),
            new_sha: zeros(),
            ref_name: "refs/heads/side".to_string(),
        };

        // The branch is still there: the claim did not land.
        assert!(!ref_update_applied(&git, "alice", "widgets", &deletion));

        // Delete it out-of-band, the way a refused-then-accepted push
        // would differ.
        let bare = dir.path().join("alice").join("widgets.git");
        run_git(
            &["--git-dir", bare.to_str().unwrap(), "branch", "-D", "side"],
            dir.path(),
        );
        assert!(ref_update_applied(&git, "alice", "widgets", &deletion));
    }
}
