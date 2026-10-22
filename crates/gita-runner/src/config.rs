//! Configuration for the gita-runner.

use clap::Parser;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser, Debug, Clone)]
#[command(name = "gita-runner", about = "Gita CI runner")]
pub struct RunnerConfig {
    /// PostgreSQL connection URL
    #[arg(long, env = "GITA_DATABASE_URL")]
    pub server_db_url: String,

    /// Base URL of the gita-server (for clone URLs and API calls)
    #[arg(long, env = "GITA_SERVER_URL", default_value = "http://localhost:3100")]
    pub server_base_url: String,

    /// Runner token (minted by `gita-server worker register`)
    #[arg(long, env = "GITA_RUNNER_TOKEN")]
    pub runner_token: String,

    /// Human-readable name for this runner instance
    #[arg(long, env = "GITA_RUNNER_NAME", default_value = "runner-1")]
    pub runner_name: String,

    /// Working directory for cloned repos and job workspaces
    #[arg(long, env = "GITA_WORK_DIR", default_value = "/tmp/gita-ci-work")]
    pub work_dir: PathBuf,

    /// How often to poll for new runs (seconds)
    #[arg(long, env = "GITA_POLL_INTERVAL", default_value = "2")]
    pub poll_interval_secs: u64,

    /// Lease duration for a claimed run (seconds)
    #[arg(long, env = "GITA_CLAIM_LEASE_SECS", default_value = "300")]
    pub claim_lease_secs: u64,

    /// Maximum concurrent jobs this runner will execute
    #[arg(long, env = "GITA_CONCURRENT_JOBS", default_value = "1")]
    pub concurrent_jobs: usize,

    /// Maximum wall-clock time a single job may run (seconds).
    ///
    /// A job script is arbitrary shell from a pushed file. Without a limit
    /// a `sleep infinity` would pin the runner forever. 0 disables the
    /// timeout.
    #[arg(long, env = "GITA_JOB_TIMEOUT_SECS", default_value = "3600")]
    pub job_timeout_secs: u64,
}

impl RunnerConfig {
    pub fn poll_interval(&self) -> std::time::Duration {
        Duration::from_secs(self.poll_interval_secs)
    }

    pub fn claim_lease(&self) -> std::time::Duration {
        Duration::from_secs(self.claim_lease_secs)
    }

    /// How long a single job may run before it is killed.
    ///
    /// `None` means "no limit" (the job timeout is disabled).
    pub fn job_timeout(&self) -> Option<Duration> {
        if self.job_timeout_secs == 0 {
            None
        } else {
            Some(Duration::from_secs(self.job_timeout_secs))
        }
    }
}
