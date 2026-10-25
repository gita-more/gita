//! Glob matching for the branch/path trigger filters.

/// Match `pat` (a glob) against `s`. `*` matches any non-`/` chars,
/// `?` matches one char. Falls back to plain equality when the
/// pattern has no glob metachars.
///
/// `literal_separator(true)` is what makes `*` stop at a `/`, which is
/// the convention every other CI platform follows and what the writing
/// guide documents. Without it globset's `*` also matches `/`, so
/// `release/*` would match the branch `release/a/b` — a branch the
/// author did not mean to select. `**` still crosses directories.
pub(crate) fn glob_match(pat: &str, s: &str) -> bool {
    if !pat.contains('*') && !pat.contains('?') && !pat.contains('[') {
        return pat == s;
    }
    globset::GlobBuilder::new(pat)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher().is_match(s))
        .unwrap_or(false)
}
