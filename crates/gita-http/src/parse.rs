//! Path parsing and read/write classification for git-over-HTTP.

/// Parse `owner/name.git/...` from a path like `/owner/name.git/info/refs`.
pub(crate) fn parse_repo_from_path(path_info: &str) -> (Option<String>, Option<String>) {
    let mut parts = path_info.trim_start_matches('/').split('/');
    let owner = parts
        .next()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let repo = parts
        .next()
        .map(|s| s.strip_suffix(".git").unwrap_or(s).to_string());
    (owner, repo)
}

/// The git service verb a request addresses, if any.
///
/// A git-over-HTTP path has the shape `/<owner>/<repo>.git/<verb>[/...]`,
/// where `<verb>` is the third path segment. Parsing the verb *structurally*
/// matters: a substring match on the whole path would classify every
/// request for a repository literally named `git-receive-pack` as a write.
pub(crate) fn request_verb(path_info: &str) -> Option<&str> {
    let mut parts = path_info.trim_start_matches('/').split('/');
    let _owner = parts.next()?;
    let _repo = parts.next()?;
    let verb = parts.next()?;
    if verb.is_empty() {
        None
    } else {
        Some(verb)
    }
}

/// Classify a git request as a read of, or a write to, the repository it
/// addresses.
///
/// The classification must be **path-structural**, not a substring match on
/// the whole path. `git http-backend` serves the "dumb" file endpoints
/// (`HEAD`, `objects/**`, `objects/info/packs`, the `.pack`/`.idx` files) when
/// `GIT_HTTP_EXPORT_ALL=1` is set — which it always is here (see
/// `gita-git::run_http_backend`). Those paths contain neither
/// `git-upload-pack` nor `info/refs`, so a substring-based check classified
/// them as "neither read nor write" and let them past the authorization gate
/// entirely. The result was that an anonymous client could download a
/// private repository's objects (and hence its whole history) while the
/// smart-HTTP path correctly answered 401.
///
/// Everything under `/<owner>/<repo>.git/` is a read of that repository;
/// the only write verb is `git-receive-pack`.
pub(crate) fn is_write_request(path_info: &str, query_string: &Option<String>) -> bool {
    if request_verb(path_info) == Some("git-receive-pack") {
        return true;
    }
    // The `service=` query parameter is how a client selects the service
    // before the path carries it (e.g. `?service=git-receive-pack`).
    query_string
        .as_deref()
        .map(|q| q.contains("service=git-receive-pack"))
        .unwrap_or(false)
}

/// Whether the request reads the repository's contents.
///
/// Conservative by design: a request that addresses the repository and is
/// not a write is treated as a read. A path we fail to recognise therefore
/// fails *closed* (a private repository still requires its owner) rather
/// than open.
pub(crate) fn is_read_request(path_info: &str, query_string: &Option<String>) -> bool {
    !is_write_request(path_info, query_string)
}
