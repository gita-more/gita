//! Path parsing and read/write classification for git-over-HTTP.

/// Parse `owner/name.git/...` from a path like `/owner/name.git/info/refs`.
/// For group repositories, owner can be a nested path like `groupa/subgroupa`.
#[allow(dead_code)]
pub(crate) fn parse_repo_from_path(path_info: &str) -> (Option<String>, Option<String>) {
    let parts = path_info
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if parts.len() < 2 {
        return (None, None);
    }
    // Find the repo segment (the one ending with .git)
    let Some(repo_idx) = parts.iter().position(|p| p.ends_with(".git")) else {
        return (None, None);
    };
    if repo_idx == 0 {
        return (None, None);
    }
    // The repo name is the .git segment (with .git stripped)
    let repo = parts[repo_idx].strip_suffix(".git").map(|s| s.to_string());
    // Everything before the repo segment is the owner/group path
    let owner = Some(parts[..repo_idx].join("/"));
    (owner, repo)
}

/// The git service verb a request addresses, if any.
///
/// A git-over-HTTP path has the shape `/<owner>/<repo>.git/<verb>[/...]`,
/// where `<verb>` is the segment AFTER the repo (which ends with `.git`).
/// For nested groups, owner can contain multiple segments, so we find the
/// repo segment (the one ending with `.git`) and take the next segment as
/// the verb. Parsing the verb *structurally* matters: a substring match on
/// the whole path would classify every request for a repository literally
/// named `git-receive-pack` as a write.
#[allow(dead_code)]
pub(crate) fn request_verb(path_info: &str) -> Option<&str> {
    let parts = path_info
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    // Find the repo segment (ends with .git)
    let repo_idx = parts.iter().position(|p| p.ends_with(".git"))?;
    // The verb is the segment immediately after the repo
    let verb_idx = repo_idx + 1;
    if verb_idx >= parts.len() {
        return None;
    }
    let verb = parts[verb_idx];
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
#[allow(dead_code)]
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
#[allow(dead_code)]
pub(crate) fn is_read_request(path_info: &str, query_string: &Option<String>) -> bool {
    !is_write_request(path_info, query_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The repo segment is the one ending in `.git`; everything before it
    /// is the owner path. A group repository has a nested owner path, so
    /// the parser must not assume exactly one owner segment.
    #[test]
    fn a_plain_owner_and_repo_are_parsed() {
        assert_eq!(
            parse_repo_from_path("/alice/widgets.git/info/refs"),
            (Some("alice".to_string()), Some("widgets".to_string()))
        );
    }

    #[test]
    fn a_nested_group_owner_is_parsed() {
        assert_eq!(
            parse_repo_from_path("/groupa/subgroupa/widgets.git/info/refs"),
            (
                Some("groupa/subgroupa".to_string()),
                Some("widgets".to_string())
            )
        );
    }

    /// A path that addresses no repository at all yields no owner and no
    /// name — the caller then treats the request as not-a-git-request.
    #[test]
    fn a_path_without_a_git_segment_is_not_a_repository() {
        assert_eq!(
            parse_repo_from_path("/alice/widgets/info/refs"),
            (None, None)
        );
    }

    /// A single segment cannot be both owner and repo.
    #[test]
    fn a_single_segment_path_is_not_a_repository() {
        assert_eq!(parse_repo_from_path("/alice"), (None, None));
    }

    /// The repo segment may not be the first one: there would be no
    /// owner to address the repository to.
    #[test]
    fn a_leading_repo_segment_has_no_owner() {
        assert_eq!(parse_repo_from_path("/widgets.git/info/refs"), (None, None));
    }

    /// The leading slash is optional — the router hands over the path
    /// with or without it.
    #[test]
    fn a_path_without_the_leading_slash_parses_the_same() {
        assert_eq!(
            parse_repo_from_path("alice/widgets.git/info/refs"),
            (Some("alice".to_string()), Some("widgets".to_string()))
        );
    }

    /// Only the segment ending in `.git` is the repository; a later
    /// segment that also ends in `.git` is not.
    #[test]
    fn the_first_git_segment_is_the_repository() {
        assert_eq!(
            parse_repo_from_path("/alice/widgets.git/objects/x.git/"),
            (Some("alice".to_string()), Some("widgets".to_string()))
        );
    }

    /// The verb is the segment *after* the repository, so a repository
    /// literally named `git-receive-pack` is not a write request.
    #[test]
    fn the_verb_is_the_segment_after_the_repository() {
        assert_eq!(
            request_verb("/alice/widgets.git/git-receive-pack"),
            Some("git-receive-pack")
        );
        // The verb is one segment: `info/refs` is spelled `info` plus
        // `refs`, and only the first segment is the verb.
        assert_eq!(
            request_verb("/alice/git-receive-pack.git/info/refs"),
            Some("info")
        );
    }

    #[test]
    fn a_path_without_a_verb_after_the_repository_has_no_verb() {
        assert_eq!(request_verb("/alice/widgets.git"), None);
        assert_eq!(request_verb("/alice/widgets.git/"), None);
    }

    /// A write is the receive-pack service, by path or by query.
    #[test]
    fn only_receive_pack_is_a_write() {
        assert!(is_write_request(
            "/alice/widgets.git/git-receive-pack",
            &None
        ));
        assert!(is_write_request(
            "/alice/widgets.git/info/refs",
            &Some("service=git-receive-pack".to_string())
        ));
        assert!(!is_write_request("/alice/widgets.git/info/refs", &None));
    }

    /// Everything else under a repository path reads the repository —
    /// including the dumb-HTTP object endpoints, which carry neither
    /// verb. Failing closed here is what keeps a private repository's
    /// objects from being served to an anonymous client.
    #[test]
    fn the_object_endpoints_read_the_repository() {
        assert!(is_read_request(
            "/alice/widgets.git/objects/info/packs",
            &None
        ));
        assert!(is_read_request("/alice/widgets.git/HEAD", &None));
    }

    /// The classification of a request is decided by the path's
    /// structure, and the path is chosen by the client.
    ///
    /// Two properties matter here, and both are about a client that
    /// picks names adversarially:
    ///
    /// * a repository whose *name* contains `git-receive-pack` is not a
    ///   write — the verb is the segment after the repository, not a
    ///   substring of the whole path;
    /// * a request that addresses a repository at all is a read unless
    ///   it is structurally a receive-pack. A path the parser fails to
    ///   understand must fail *closed*, never open.
    mod arbitrary_paths {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(1024))]

            /// A repository named after the write verb is still a
            /// repository, and a request for it is a read.
            #[test]
            fn a_repository_named_after_the_write_verb_is_not_a_write(
                name in "[a-z]{1,12}",
            ) {
                let path = format!("/{name}/git-receive-pack.git/info/refs");
                prop_assert!(
                    !is_write_request(&path, &None),
                    "{} was classified as a write", path
                );
                prop_assert!(is_read_request(&path, &None));
            }

            /// The verb is the segment *after* the repository, so a
            /// write needs one there.
            #[test]
            fn only_a_verb_after_the_repository_is_a_write(
                owner in "[a-z]{1,10}",
                repo in "[a-z]{1,10}",
                tail in "[a-z/]{0,20}",
            ) {
                let path = format!("/{owner}/{repo}.git/{tail}");
                let is_write = is_write_request(&path, &None);
                prop_assert_eq!(
                    is_write,
                    request_verb(&path) == Some("git-receive-pack"),
                    "{} was classified inconsistently", path
                );
            }

            /// A path the parser cannot make sense of is a read, never
            /// an open door. Failing closed is what keeps a private
            /// repository private.
            #[test]
            fn an_unrecognised_path_fails_closed(
                path in "/[a-zA-Z0-9._/-]{0,80}",
            ) {
                let (owner, repo) = parse_repo_from_path(&path);
                if owner.is_none() && repo.is_none() {
                    // The path addresses no repository, so the
                    // classification carries no repository to leak.
                    return Ok(());
                }
                prop_assert!(
                    is_read_request(&path, &None),
                    "{} addresses a repository but was not treated as a read", path
                );
            }

            /// The owner path may be nested, and the repository is
            /// always the segment that ends in `.git`.
            #[test]
            fn the_repository_is_the_dot_git_segment(
                owner in "[a-z]{1,8}",
                repo in "[a-z]{1,8}",
                verb in "[a-z-]{1,12}",
            ) {
                let path = format!("/{owner}/{repo}.git/{verb}");
                let (parsed_owner, parsed_repo) = parse_repo_from_path(&path);
                prop_assert_eq!(parsed_owner.as_deref(), Some(owner.as_str()));
                prop_assert_eq!(parsed_repo.as_deref(), Some(repo.as_str()));
                prop_assert_eq!(request_verb(&path), Some(verb.as_str()));
            }
        }
    }
}
