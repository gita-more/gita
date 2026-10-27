//! Workspace management: cloning, per-job workdirs, and cleanup.

use crate::config::RunnerConfig;
use std::path::PathBuf;

/// Layout:
///   <work_dir>/ci-work/<run_id>/repo/        # bare clone of the repo at head_sha
///   <work_dir>/ci-work/<run_id>/<job_name>/  # per-job workspace (copy of repo)

pub fn run_work_dir(config: &RunnerConfig, run_id: i64) -> PathBuf {
    config.work_dir.join("ci-work").join(run_id.to_string())
}

pub fn job_work_dir(config: &RunnerConfig, run_id: i64, job_name: &str) -> PathBuf {
    let safe: String = job_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    config
        .work_dir
        .join("ci-work")
        .join(run_id.to_string())
        .join(&safe)
}

fn base_host(config: &RunnerConfig) -> &str {
    config
        .server_base_url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
}

/// Build the clone URL with the runner token as Basic auth password.
/// git clone https://x-access-token:<TOKEN>@<host>/<owner>/<repo>.git
///
/// Git smart-HTTP is mounted at the root path `/{owner}/{repo}.git/...`,
/// matching GitLab/GitHub's clean clone URLs.
///
/// A `file://` server base is passed through unchanged, without the token,
/// so a runner can be pointed at a local bare repository. That is what the
/// integration tests use to exercise the real clone path.
pub fn clone_url(config: &RunnerConfig, owner: &str, repo: &str) -> String {
    if config.server_base_url.starts_with("file://") {
        let base = config.server_base_url.trim_end_matches('/');
        return format!("{base}/{owner}/{repo}.git");
    }
    format!(
        "https://x-access-token:{}@{}/{}/{}.git",
        config.runner_token,
        base_host(config),
        owner,
        repo
    )
}
