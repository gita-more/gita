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
    Some((
        match verb {
            "upload-pack" => "upload-pack",
            "receive-pack" => "receive-pack",
            _ => return None,
        },
        path.to_string(),
    ))
}
