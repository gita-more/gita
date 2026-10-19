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
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    config.work_dir.join("ci-work").join(run_id.to_string()).join(&safe)
}

fn base_host(config: &RunnerConfig) -> &str {
    config
        .server_base_url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
}

/// Build the clone URL with the runner token as Basic auth password.
/// git clone https://x-access-token:<TOKEN>@<host>/<owner>/<repo>.git
pub fn clone_url(config: &RunnerConfig, owner: &str, repo: &str) -> String {
    format!(
        "https://x-access-token:{}@{}",
        config.runner_token,
        format!("{}/{}.git", base_host(config), format!("{}/{}", owner, repo))
    )
}