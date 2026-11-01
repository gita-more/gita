//! The runner's client for the gita server.
//!
//! The runner used to talk to Postgres directly: it claimed runs with a
//! `FOR UPDATE SKIP LOCKED` query of its own, wrote job rows, log lines
//! and status updates straight into the tables, and handed its connection
//! pool to the VM store. Every one of those operations now goes through
//! the server's API instead, so:
//!
//! * the runner needs no database credentials and no network access to
//!   the database — it needs a URL and a token;
//! * the schema stays private to the server;
//! * the runner can run on a different machine, or on a different
//!   network, than the database.
//!
//! Authentication is a **runner token**: a personal access token whose
//! stored name starts with `runner/`. The server rejects anything else.

use anyhow::{Context, Result};
use gita_db::{JobRow, PipelineRow, PipelineRunRow};
use serde::{Deserialize, Serialize};

/// A log line on its way to the server.
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub seq: i64,
    /// `"stdout"` or `"stderr"`.
    pub stream: String,
    pub line: String,
    pub timestamp: String,
}

/// What a claim returned.
#[derive(Debug, Clone, Deserialize)]
pub struct ClaimedRun {
    /// The run, or `None` when there was nothing to claim.
    pub run: Option<PipelineRunRow>,
    /// The pipeline the run executes: its `path` decides whether the run
    /// is a `.gita-ci.yml` pipeline or a GitHub-style workflow, and its
    /// `content` is the YAML the runner has to execute.
    pub pipeline: Option<PipelineRow>,
}

/// The repository a run belongs to.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoRef {
    pub owner_name: String,
    pub name: String,
}

/// A VM created for a CI job.
#[derive(Debug, Clone, Deserialize)]
pub struct CiVm {
    pub vm_id: i64,
}

/// The client the runner uses to talk to the server.
#[derive(Debug, Clone)]
pub struct RunnerClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl RunnerClient {
    /// Build a client for `base_url`, authenticating with `token`.
    pub fn new(base_url: &str, token: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .context("build HTTP client")?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
        })
    }

    /// The server this client talks to.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, self.url(path))
            .bearer_auth(&self.token)
    }

    /// Turn a non-success response into an error that says what the
    /// server said — a bare status code is useless at 2am.
    async fn check(&self, response: reqwest::Response, what: &str) -> Result<reqwest::Response> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        Err(anyhow::anyhow!(
            "{what} failed: HTTP {}: {}",
            status.as_u16(),
            body.trim()
        ))
    }

    // ------------------------------------------------------------------
    // Runs
    // ------------------------------------------------------------------

    /// Claim the next queued run, if there is one.
    pub async fn claim_run(
        &self,
        runner_name: &str,
        lease_secs: u64,
    ) -> Result<Option<ClaimedRun>> {
        #[derive(Serialize)]
        struct ClaimRequest<'a> {
            runner_name: &'a str,
            lease_secs: i64,
        }
        let response = self
            .request(reqwest::Method::POST, "/api/runner/claim")
            .json(&ClaimRequest {
                runner_name,
                lease_secs: lease_secs as i64,
            })
            .send()
            .await
            .context("claim run")?;
        let response = self.check(response, "claim").await?;
        let claimed: ClaimedRun = response.json().await.context("decode claim response")?;
        Ok(claimed.run.map(|run| ClaimedRun {
            run: Some(run),
            pipeline: claimed.pipeline,
        }))
    }

    /// The current state of a run — used to notice a cancellation.
    pub async fn get_run(&self, run_id: i64) -> Result<PipelineRunRow> {
        let response = self
            .request(reqwest::Method::GET, &format!("/api/runner/runs/{run_id}"))
            .send()
            .await
            .context("get run")?;
        let response = self.check(response, "get run").await?;
        Ok(response.json().await.context("decode run")?)
    }

    /// Set a run's status (`running`, `success`, `failed`, `canceled`).
    pub async fn set_run_status(&self, run_id: i64, status: &str) -> Result<()> {
        #[derive(Serialize)]
        struct StatusRequest<'a> {
            status: &'a str,
        }
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("/api/runner/runs/{run_id}/status"),
            )
            .json(&StatusRequest { status })
            .send()
            .await
            .context("set run status")?;
        self.check(response, "set run status").await?;
        Ok(())
    }

    /// Create a job row and return its id.
    pub async fn create_job(
        &self,
        run_id: i64,
        name: &str,
        stage: &str,
        needs: &[String],
        script: &[String],
    ) -> Result<i64> {
        #[derive(Serialize)]
        struct CreateJobRequest<'a> {
            name: &'a str,
            stage: &'a str,
            needs: &'a [String],
            script: &'a [String],
        }
        #[derive(Deserialize)]
        struct CreateJobResponse {
            job_id: i64,
        }
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("/api/runner/runs/{run_id}/jobs"),
            )
            .json(&CreateJobRequest {
                name,
                stage,
                needs,
                script,
            })
            .send()
            .await
            .context("create job")?;
        let response = self.check(response, "create job").await?;
        let created: CreateJobResponse = response.json().await.context("decode job")?;
        Ok(created.job_id)
    }

    // ------------------------------------------------------------------
    // Jobs
    // ------------------------------------------------------------------

    /// The current state of a job.
    pub async fn get_job(&self, job_id: i64) -> Result<JobRow> {
        let response = self
            .request(reqwest::Method::GET, &format!("/api/runner/jobs/{job_id}"))
            .send()
            .await
            .context("get job")?;
        let response = self.check(response, "get job").await?;
        Ok(response.json().await.context("decode job")?)
    }

    /// Set a job's status.
    pub async fn set_job_status(
        &self,
        job_id: i64,
        status: &str,
        exit_code: Option<i32>,
    ) -> Result<()> {
        #[derive(Serialize)]
        struct StatusRequest<'a> {
            status: &'a str,
            exit_code: Option<i32>,
        }
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("/api/runner/jobs/{job_id}/status"),
            )
            .json(&StatusRequest { status, exit_code })
            .send()
            .await
            .context("set job status")?;
        self.check(response, "set job status").await?;
        Ok(())
    }

    /// Append log lines to a job.
    ///
    /// Logs go out in batches: a chatty build would otherwise cost one
    /// HTTP request per line. The server's insert is idempotent per
    /// `(job_id, seq)`, so a retried batch does not duplicate anything.
    pub async fn append_logs(&self, job_id: i64, lines: &[LogLine]) -> Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("/api/runner/jobs/{job_id}/logs"),
            )
            .json(lines)
            .send()
            .await
            .context("append logs")?;
        self.check(response, "append logs").await?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Repositories
    // ------------------------------------------------------------------

    /// The owner and name of a repository.
    pub async fn get_repo(&self, repo_id: i64) -> Result<RepoRef> {
        let response = self
            .request(
                reqwest::Method::GET,
                &format!("/api/runner/repos/{repo_id}"),
            )
            .send()
            .await
            .context("get repository")?;
        let response = self.check(response, "get repository").await?;
        Ok(response.json().await.context("decode repository")?)
    }

    // ------------------------------------------------------------------
    // VMs
    // ------------------------------------------------------------------

    /// Ask the server's VM control plane for a VM for this job.
    ///
    /// The server resolves the template, writes the VM row linked to the
    /// job, stores the agent token hash and hands the job's user data to
    /// the VM. The runner never touches the VM tables.
    pub async fn create_ci_vm(
        &self,
        run_id: i64,
        job_id: i64,
        repo_id: i64,
        template: &str,
        cpu_millis: Option<u64>,
        memory_bytes: Option<u64>,
        disk_bytes: Option<u64>,
        expires_at_ms: Option<i64>,
        user_data: Option<String>,
        agent_token_hash: &str,
    ) -> Result<CiVm> {
        #[derive(Serialize)]
        struct CreateVmRequest<'a> {
            run_id: i64,
            job_id: i64,
            repo_id: i64,
            template: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            cpu_millis: Option<u64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            memory_bytes: Option<u64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            disk_bytes: Option<u64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            expires_at_ms: Option<i64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            user_data: Option<String>,
            agent_token_hash: &'a str,
        }
        let response = self
            .request(reqwest::Method::POST, "/api/v1/vm/ci/create")
            .json(&CreateVmRequest {
                run_id,
                job_id,
                repo_id,
                template,
                cpu_millis,
                memory_bytes,
                disk_bytes,
                expires_at_ms,
                user_data,
                agent_token_hash,
            })
            .send()
            .await
            .context("create CI vm")?;
        let response = self.check(response, "create CI vm").await?;
        Ok(response.json().await.context("decode CI vm")?)
    }

    /// Ask the server to destroy a VM that belongs to this job.
    pub async fn reclaim_ci_vm(&self, vm_id: i64, job_id: i64) -> Result<()> {
        #[derive(Serialize)]
        struct ReclaimRequest {
            vm_id: i64,
            job_id: i64,
        }
        let response = self
            .request(reqwest::Method::POST, "/api/v1/vm/ci/reclaim")
            .json(&ReclaimRequest { vm_id, job_id })
            .send()
            .await
            .context("reclaim CI vm")?;
        self.check(response, "reclaim CI vm").await?;
        Ok(())
    }
}
