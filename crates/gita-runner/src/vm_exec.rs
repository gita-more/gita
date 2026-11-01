//! VM-backed job execution.
//!
//! A job declared with a `vm:` block does not run on the runner host. The
//! runner provisions a virtual machine from a cluster template, injects
//! the job's execution material through cloud-init, and the one-shot
//! agent inside the VM runs the script and reports back. The runner
//! watches the job row until the agent (or a timeout, or a cancel)
//! settles it, then reclaims the VM.
//!
//! Security notes:
//!
//! - The per-job agent token is minted here and only its hash lands in
//!   the database; the plaintext goes into the VM's user data and dies
//!   with the VM.
//! - The runner's own credentials never enter the VM. A private repo
//!   cannot be cloned by the VM today (the anonymous HTTP clone fails) —
//!   the job fails with a clear message rather than leaking the runner
//!   token into user data.
//! - The VM carries an expiry (`expires_at_ms`) past which the control
//!   plane's GC reclaims it even if this runner process dies mid-job.

use crate::client::RunnerClient;
use crate::config::RunnerConfig;
use anyhow::{Context, Result};
use rand::RngCore;
use std::time::Duration;
use tokio::time::{interval, MissedTickBehavior};

/// How often to check the job row while the VM works.
const WATCH_POLL: Duration = Duration::from_secs(2);

/// Outcome of a VM job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmJobOutcome {
    Success,
    Failed,
    Canceled,
    /// The watcher gave up (agent never reported, VM never came up...).
    TimedOut,
    /// The VM infrastructure itself failed (no template, no capacity...).
    InfrastructureError,
}

impl VmJobOutcome {
    /// Whether the job succeeded — the shape `execute_pipeline` expects.
    pub fn ok(&self) -> bool {
        matches!(self, VmJobOutcome::Success)
    }
}

/// Run `job` inside a cluster VM and wait for it to settle.
///
/// `job_id` must already exist (queued). `vm` is the parsed `vm:` block
/// from `.gita-ci.yml`.
pub async fn run_job_in_vm(
    client: &RunnerClient,
    config: &RunnerConfig,
    run: &gita_db::PipelineRunRow,
    job_id: i64,
    vm: &gita_pipeline::VmJob,
    job_script: &str,
) -> Result<VmJobOutcome> {
    // 1. Mint the per-job agent token: plaintext goes into the VM, only
    //    the hash is stored — and the hash is stored by the *server*, as
    //    part of the create request.
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let agent_token = hex::encode(bytes);
    let agent_token_hash = gita_db::hash_token(&agent_token);

    // 2. Validate the sizes from the job's `vm:` block. They were
    //    validated at parse time, so these parses only fail on a
    //    programmer error — but map them to a job failure regardless
    //    rather than panicking a runner.
    let memory_bytes = match &vm.memory {
        Some(s) => match gita_pipeline::VmJob::parse_size(s) {
            Ok(b) => Some(b),
            Err(e) => {
                let msg = format!("invalid vm memory size {s:?}: {e}");
                tracing::warn!(job = job_id, "{msg}");
                write_system_log(client, job_id, &msg).await?;
                return Ok(VmJobOutcome::InfrastructureError);
            }
        },
        None => None,
    };
    let disk_bytes = match &vm.disk {
        Some(s) => match gita_pipeline::VmJob::parse_size(s) {
            Ok(b) => Some(b),
            Err(e) => {
                let msg = format!("invalid vm disk size {s:?}: {e}");
                tracing::warn!(job = job_id, "{msg}");
                write_system_log(client, job_id, &msg).await?;
                return Ok(VmJobOutcome::InfrastructureError);
            }
        },
        None => None,
    };

    // 4. Compose the agent's user data. The job's environment variables
    //    (GITA_REPOSITORY etc.) are exported in the script preamble so
    //    VM jobs see the same surface local jobs do.
    let repo_label = repo_full_name(client, run).await?;
    let script = format!(
        // The agent runs `bash <file>` without -e (its own -ec applies to
        // the outer shell only), so the file aborts on the first failing
        // command itself — the same semantics as the local executor's
        // `set -e`.
        "set -e\nexport GITA_REPOSITORY={repo}\nexport GITA_COMMIT_SHA={sha}\nexport GITA_REF={ref_}\nexport GITA_JOB_NAME={job_name}\nexport GITA_RUN_ID={run_id}\n{script}",
        repo = repo_label,
        sha = run.head_sha,
        ref_ = run.ref_name,
        job_name = job_name_for(client, job_id).await,
        run_id = run.id,
        script = job_script,
    );
    let clone_url = format!(
        "{}/{}.git",
        config.server_base_url.trim_end_matches('/'),
        repo_label
    );
    let userdata = gita_ci::generate_vm_job_userdata(
        &config.server_base_url,
        job_id,
        &agent_token,
        &clone_url,
        &run.head_sha,
        &script,
        // No SSH key: private-repo VM jobs are not supported yet. The
        // runner token must never enter a VM.
        "",
    )
    .context("generate vm job user data")?;
    // 5. Provision the VM — the server's control plane does the work:
    //    it resolves the template, writes the VM row linked to this job,
    //    and stores the agent token hash. The expiry gives the
    //    control-plane GC a backstop: if this runner dies, the VM is
    //    still reclaimed.
    let timeout = config.job_timeout().unwrap_or(Duration::from_secs(3600));
    let expires_at_ms = Some(now_ms() + timeout.as_millis() as i64 + 600_000);
    let vm_result = match client
        .create_ci_vm(
            run.id,
            job_id,
            run.repo_id,
            &vm.template,
            vm.cpu.map(|cores| cores * 1000),
            memory_bytes,
            disk_bytes,
            expires_at_ms,
            Some(userdata),
            &agent_token_hash,
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("failed to provision VM: {e}");
            tracing::warn!(job = job_id, "{msg}");
            write_system_log(client, job_id, &msg).await?;
            return Ok(VmJobOutcome::InfrastructureError);
        }
    };
    tracing::info!(job = job_id, vm = vm_result.vm_id, "provisioned VM for job");

    // 6. Watch the job row until it settles. The agent moves it
    //    queued → running → success/failed; a user cancel also lands
    //    here.
    let outcome = watch_job(client, job_id, timeout).await;

    // 7. Reclaim the VM, whatever happened.
    if let Err(e) = client.reclaim_ci_vm(vm_result.vm_id, job_id).await {
        tracing::warn!(job = job_id, vm = vm_result.vm_id, "reclaim VM failed: {e}");
    }

    Ok(outcome)
}

/// Poll the job row until a terminal state or the timeout.
async fn watch_job(client: &RunnerClient, job_id: i64, timeout: Duration) -> VmJobOutcome {
    let mut ticker = interval(WATCH_POLL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        ticker.tick().await;

        match client.get_job(job_id).await {
            Ok(job) => match job.status.as_str() {
                "success" => return VmJobOutcome::Success,
                "failed" => return VmJobOutcome::Failed,
                "canceled" => return VmJobOutcome::Canceled,
                _ => {}
            },
            Err(e) => tracing::warn!(job = job_id, "watch poll failed: {e}"),
        }

        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(job = job_id, "vm job exceeded its timeout");
            return VmJobOutcome::TimedOut;
        }
    }
}

/// Best-effort system log line, so the failure is visible in the job's
/// log view and not just the runner's stderr.
async fn write_system_log(client: &RunnerClient, job_id: i64, line: &str) -> Result<()> {
    let _ = client
        .append_logs(
            job_id,
            &[crate::client::LogLine {
                seq: 0,
                stream: "system".to_string(),
                line: line.to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
            }],
        )
        .await;
    Ok(())
}

/// The current time in unix milliseconds.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

async fn repo_full_name(client: &RunnerClient, run: &gita_db::PipelineRunRow) -> Result<String> {
    match client.get_repo(run.repo_id).await {
        Ok(repo) => Ok(format!("{}/{}", repo.owner_name, repo.name)),
        Err(e) => Err(anyhow::anyhow!("could not resolve the repository: {e:#}")),
    }
}

async fn job_name_for(client: &RunnerClient, job_id: i64) -> String {
    client
        .get_job(job_id)
        .await
        .map(|j| j.name)
        .unwrap_or_default()
}
