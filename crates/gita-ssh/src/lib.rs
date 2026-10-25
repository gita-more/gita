//! Embedded SSH server for the Git protocol.
//!
//! Listens for SSH connections from `git@host:owner/repo.git` clients and
//! runs the corresponding `git-upload-pack` / `git-receive-pack` child
//! process with its stdio wired to the SSH channel. Public-key auth is
//! looked up against the `ssh_keys` table; an unknown key disconnects.
//!
//! Wire protocol: clients connect via `ssh://` or the scp-style `git@host:`
//! URLs. The server accepts only `exec` channels (no shell, no PTY).

pub mod config;
mod handler;
mod path;
pub mod server;

pub use config::{load_or_create_host_key, SshServerConfig, DEFAULT_HOST_KEY_NAME};
pub use server::{start, SshServerHandle};

/// How much of the client's request to keep for the ref-update parser.
///
/// The commands arrive first and are followed by the pack data, so the
/// buffer is capped: the parser stops at the flush packet that ends the
/// command list, and the pack data behind it is not needed.
pub const MAX_CAPTURED_COMMANDS: usize = 64 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{parse_git_command, resolve_repo_path};
    use russh::keys::ssh_key;
    use std::path::PathBuf;

    #[test]
    fn host_key_is_generated_and_reloaded() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ssh_host_ed25519_key");

        let key = load_or_create_host_key(&path).unwrap();
        // The file must be created.
        assert!(path.exists());
        // Reloading returns an identical key (same host identity).
        let reloaded = load_or_create_host_key(&path).unwrap();
        assert_eq!(
            key.public_key()
                .fingerprint(ssh_key::HashAlg::Sha256)
                .to_string(),
            reloaded
                .public_key()
                .fingerprint(ssh_key::HashAlg::Sha256)
                .to_string(),
        );
    }

    #[test]
    fn host_key_rejects_invalid_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("bad_key");
        std::fs::write(&path, "not a key").unwrap();
        assert!(load_or_create_host_key(&path).is_err());
    }

    #[test]
    fn parse_git_command_handles_common_forms() {
        assert_eq!(
            parse_git_command("git-upload-pack 'owner/repo.git'"),
            Some(("upload-pack", "owner/repo.git".to_string()))
        );
        assert_eq!(
            parse_git_command("git-receive-pack '/srv/git/owner/repo.git'"),
            Some(("receive-pack", "/srv/git/owner/repo.git".to_string()))
        );
        assert_eq!(
            parse_git_command("git-upload-pack owner/repo.git"),
            Some(("upload-pack", "owner/repo.git".to_string()))
        );
        assert_eq!(parse_git_command("ls"), None);
        assert_eq!(parse_git_command("git-upload-pack"), None);
        // Trailing NUL is tolerated (some clients append it).
        assert_eq!(
            parse_git_command("git-upload-pack 'owner/repo.git'\0"),
            Some(("upload-pack", "owner/repo.git".to_string()))
        );
    }

    #[test]
    fn resolve_repo_path_accepts_valid_owner_repo() {
        let root = PathBuf::from("/data/repos");

        let p = resolve_repo_path(&root, "alice/hello.git").unwrap();
        assert_eq!(p, root.join("alice").join("hello.git"));
        // Leading slash and .git suffix variations.
        let p2 = resolve_repo_path(&root, "/alice/hello.git").unwrap();
        assert_eq!(p2, root.join("alice").join("hello.git"));
        let p3 = resolve_repo_path(&root, "/alice/hello").unwrap();
        assert_eq!(p3, root.join("alice").join("hello.git"));
    }

    #[test]
    fn resolve_repo_path_rejects_traversal_and_malformed() {
        let root = PathBuf::from("/data/repos");

        // Path traversal attempts.
        assert!(resolve_repo_path(&root, "../secret.git").is_none());
        assert!(resolve_repo_path(&root, "../../etc/passwd.git").is_none());
        assert!(resolve_repo_path(&root, "a/../../b").is_none());
        // Missing owner or repo.
        assert!(resolve_repo_path(&root, "alice.git").is_none());
        assert!(resolve_repo_path(&root, "alice").is_none());
        assert!(resolve_repo_path(&root, "").is_none());
        // Too many segments.
        assert!(resolve_repo_path(&root, "a/b/c.git").is_none());
        // Windows separators / drive letters.
        assert!(resolve_repo_path(&root, "a\\b\\c.git").is_none());
        assert!(resolve_repo_path(&root, "C:/x.git").is_none());
    }
}
