//! The `GITHUB_*` environment a workflow job sees.
//!
//! Workflow jobs do not run here — the runner builds one shell program per
//! job and executes it inside a VM (see [`crate::workflow_run`]). What both
//! sides need to agree on is the environment the steps observe, so the
//! value set lives here rather than being duplicated in the script builder.

use std::collections::BTreeMap;

/// Standard `GITHUB_*` environment variables injected into every step.
///
/// The set mirrors what GitHub Actions exposes, restricted to values gita
/// knows about. Values are provided by the caller so the builder stays
/// generic; the `GITHUB_` prefix is constant.
pub fn github_env(
    repo: &str,
    run_id: &str,
    job_name: &str,
    commit_sha: &str,
    ref_name: &str,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("GITHUB_REPOSITORY".to_string(), repo.to_string());
    env.insert("GITHUB_RUN_ID".to_string(), run_id.to_string());
    env.insert("GITHUB_JOB".to_string(), job_name.to_string());
    env.insert("GITHUB_SHA".to_string(), commit_sha.to_string());
    env.insert("GITHUB_REF".to_string(), ref_name.to_string());
    env.insert("GITHUB_WORKSPACE".to_string(), "/workspace".to_string());
    env.insert("CI".to_string(), "true".to_string());
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_the_github_names() {
        let env = github_env("o/r", "7", "build", "abc", "refs/heads/main");
        assert_eq!(env["GITHUB_REPOSITORY"], "o/r");
        assert_eq!(env["GITHUB_RUN_ID"], "7");
        assert_eq!(env["GITHUB_JOB"], "build");
        assert_eq!(env["GITHUB_SHA"], "abc");
        assert_eq!(env["GITHUB_REF"], "refs/heads/main");
        assert_eq!(env["CI"], "true");
    }
}
