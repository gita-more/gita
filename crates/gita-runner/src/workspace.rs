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
    // The trailing slash of a configured base URL is dropped: keeping it
    // would produce `host//owner/repo.git`, which git refuses to clone.
    let host = base_host(config).trim_end_matches('/');
    format!(
        "https://x-access-token:{}@{}/{}/{}.git",
        config.runner_token, host, owner, repo
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RunnerConfig;

    fn config(base: &str, token: &str) -> RunnerConfig {
        RunnerConfig {
            server_db_url: "postgres://localhost/gita".to_string(),
            server_base_url: base.to_string(),
            runner_token: token.to_string(),
            runner_name: "runner-1".to_string(),
            work_dir: PathBuf::from("/tmp/gita-ci-work"),
            poll_interval_secs: 2,
            claim_lease_secs: 300,
            concurrent_jobs: 1,
            job_timeout_secs: 3600,
        }
    }

    #[test]
    fn the_work_dirs_are_derived_from_the_run_and_the_job() {
        let c = config("http://localhost:3100", "tok");
        assert_eq!(
            run_work_dir(&c, 42),
            PathBuf::from("/tmp/gita-ci-work/ci-work/42")
        );
        assert_eq!(
            job_work_dir(&c, 42, "build"),
            PathBuf::from("/tmp/gita-ci-work/ci-work/42/build")
        );
    }

    /// A job name comes from a pushed YAML file. It must not be able to
    /// point the workspace outside the run's own directory.
    #[test]
    fn a_job_name_stays_inside_the_run_directory() {
        let c = config("http://localhost:3100", "tok");
        let run_dir = run_work_dir(&c, 7);

        for name in ["../../etc", "../..", "/absolute", "a/b", "with space", "."] {
            let dir = job_work_dir(&c, 7, name);
            assert!(
                dir.starts_with(&run_dir) && dir.parent() == Some(run_dir.as_path()),
                "job name {name:?} escaped the run directory: {dir:?}"
            );
        }
    }

    #[test]
    fn a_job_name_is_flattened_into_one_segment() {
        let c = config("http://localhost:3100", "tok");
        // Every character that is not alphanumeric, `_` or `-` becomes
        // `_` — including the dots and the slashes.
        assert_eq!(
            job_work_dir(&c, 1, "../../etc"),
            PathBuf::from("/tmp/gita-ci-work/ci-work/1/______etc")
        );
        assert_eq!(
            job_work_dir(&c, 1, "build-linux-amd64"),
            PathBuf::from("/tmp/gita-ci-work/ci-work/1/build-linux-amd64")
        );
    }

    /// Git smart-HTTP is mounted at the root path, so the clone URL has
    /// no `/git` prefix — and the runner token travels as the Basic-auth
    /// password.
    #[test]
    fn the_clone_url_carries_the_token_and_has_no_git_prefix() {
        let c = config("http://localhost:3100", "secret-token");
        assert_eq!(
            clone_url(&c, "alice", "widgets"),
            "https://x-access-token:secret-token@localhost:3100/alice/widgets.git"
        );

        let c = config("https://gita.example.com/", "secret-token");
        assert_eq!(
            clone_url(&c, "alice", "widgets"),
            "https://x-access-token:secret-token@gita.example.com/alice/widgets.git"
        );
    }

    /// A `file://` remote carries no token: it is a local bare
    /// repository, and the token would only leak into the clone URL.
    #[test]
    fn a_file_remote_is_cloned_without_the_token() {
        let c = config("file:///tmp/gita/repos", "secret-token");
        assert_eq!(
            clone_url(&c, "alice", "widgets"),
            "file:///tmp/gita/repos/alice/widgets.git"
        );
    }
}
