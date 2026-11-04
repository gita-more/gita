//! Path resolution and command parsing for SSH git access.

use std::path::PathBuf;

/// Resolve an SSH git path (`owner/repo.git`, `/owner/repo.git`, with or
/// without a trailing `.git`) into an on-disk bare repository path,
/// refusing path traversal and malformed names.
pub(crate) fn resolve_repo_path(repos_root: &std::path::Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim_start_matches('/');
    let rel = rel.strip_suffix(".git").unwrap_or(rel);
    let mut parts = rel.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    // Reject extra path segments and traversal.
    if parts.next().is_some() {
        return None;
    }
    if owner.is_empty()
        || repo.is_empty()
        || owner.contains("..")
        || repo.contains("..")
        || owner.contains('\\')
        || repo.contains('\\')
        || owner.contains(':')
        || repo.contains(':')
    {
        return None;
    }
    Some(repos_root.join(owner).join(format!("{repo}.git")))
}

/// Parse an SSH exec command into a `(verb, path)` pair.
///
/// Only `git-upload-pack` and `git-receive-pack` are accepted; everything
/// else (a shell, a PTY, arbitrary commands) is rejected.
pub(crate) fn parse_git_command(command: &str) -> Option<(&'static str, String)> {
    let s = command.trim().trim_end_matches('\0');
    let stripped = s.strip_prefix("git-")?;
    let (verb, rest) = if let Some(rest) = stripped.strip_prefix("upload-pack ") {
        ("upload-pack", rest)
    } else if let Some(rest) = stripped.strip_prefix("receive-pack ") {
        ("receive-pack", rest)
    } else {
        return None;
    };
    let path = rest.trim();
    let path = path
        .strip_prefix('\'')
        .and_then(|p| p.strip_suffix('\''))
        .unwrap_or(path);
    // An empty path cannot name a repository; refuse it here rather than
    // letting it travel further as `Some("")`.
    if path.is_empty() {
        return None;
    }
    Some((
        match verb {
            "upload-pack" => "upload-pack",
            "receive-pack" => "receive-pack",
            _ => return None,
        },
        path.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The repository path is built from a name the client chose, so
    /// the only thing standing between a client and the whole disk is
    /// this function.
    ///
    /// These tests pin the property that matters: whatever the client
    /// sends, a resolved path stays under the repositories root. A
    /// client that sends `../../etc` gets nothing at all.
    mod traversal {
        use super::*;

        fn root() -> std::path::PathBuf {
            std::path::PathBuf::from("/srv/repos")
        }

        // An accepted path never leaves the repositories root.
        //
        // The check is done on the *components* of the resolved path,
        // because a textual check would be fooled by a name that
        // contains `..` without being one (`a..b`).
        proptest! {
            #![proptest_config(ProptestConfig::with_cases(1024))]

            #[test]
            fn an_accepted_path_stays_under_the_root(
                owner in ".{0,40}",
                repo in ".{0,40}",
            ) {
                let rel = format!("{owner}/{repo}");
                let Some(path) = resolve_repo_path(&root(), &rel) else {
                    return Ok(());
                };

                // Every component of the resolved path must be a plain
                // name: no `..`, no root, nothing that steps out of the
                // directory.
                for component in path.components() {
                    prop_assert!(
                        !matches!(component, std::path::Component::ParentDir),
                        "the resolved path {:?} steps out of the root",
                        path
                    );
                }

                // And it must actually be under the root.
                prop_assert!(
                    path.starts_with(root()),
                    "the resolved path {:?} is not under the root",
                    path
                );
            }
        }

        /// A name that tries to climb out is refused outright.
        #[test]
        fn a_climbing_name_is_refused() {
            for rel in [
                "../etc/passwd",
                "owner/../../etc",
                "..",
                "owner/..",
                "/../etc",
                "owner/../..",
            ] {
                assert!(
                    resolve_repo_path(&root(), rel).is_none(),
                    "{rel:?} must not resolve to a path"
                );
            }
        }

        /// A path with more than `owner/repo` is refused: the protocol
        /// has no sub-resources below a repository.
        #[test]
        fn a_deeper_path_is_refused() {
            assert!(resolve_repo_path(&root(), "owner/repo/extra").is_none());
            assert!(resolve_repo_path(&root(), "owner/repo/extra/more").is_none());
        }

        /// The ordinary case still works, with and without the `.git`
        /// suffix and with or without a leading slash.
        #[test]
        fn an_ordinary_path_resolves() {
            let expected = root().join("alice").join("widgets.git");
            assert_eq!(
                resolve_repo_path(&root(), "alice/widgets"),
                Some(expected.clone())
            );
            assert_eq!(
                resolve_repo_path(&root(), "alice/widgets.git"),
                Some(expected.clone())
            );
            assert_eq!(
                resolve_repo_path(&root(), "/alice/widgets.git"),
                Some(expected)
            );
        }
    }
}
