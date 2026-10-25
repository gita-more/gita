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
