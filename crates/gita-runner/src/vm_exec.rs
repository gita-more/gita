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

use crate::config::RunnerConfig;
use anyhow::{Context, Result};
use gita_db::Db;
use gita_vm::{CreateVmOptions, VmSpecOverrides};
use gita_vm_core::repository::Repository;
use gita_vm_core::Result as VmResult;
use gita_vm_store::VmStore;
use rand::RngCore;
use std::sync::Arc;
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
    db: &Db,
    config: &RunnerConfig,
    run: &gita_db::PipelineRunRow,
    job_id: i64,
    vm: &gita_pipeline::VmJob,
    job_script: &str,
) -> Result<VmJobOutcome> {
    // 1. Mint the per-job agent token: plaintext goes into the VM, only
    //    the hash is stored.
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let agent_token = hex::encode(bytes);
    db.set_job_agent_token(job_id, &gita_db::hash_token(&agent_token))
        .await
        .context("record agent token")?;

    // 2. Resolve the template by name. The whole cluster's template list
    //    is small; filter in memory rather than adding a by-name query
    //    that only this caller uses.
    let store = Arc::new(VmStore::new(db.pool().clone()));
    let templates = store.list_templates(None).await?;
    let Some(template) = templates.iter().find(|t| t.name == vm.template) else {
        let msg = format!(
            "vm template {:?} not found — ask an administrator to create it (gita-admin vm template)",
            vm.template
        );
        tracing::warn!(job = job_id, "{msg}");
        write_system_log(db, job_id, &msg).await?;
        return Ok(VmJobOutcome::InfrastructureError);
    };

    // 3. Build the overrides from the job's `vm:` block. Sizes were
    //    validated at parse time, so these parses only fail on a
    //    programmer error — but map them to a job failure regardless
    //    rather than panicking a runner.
    let overrides = VmSpecOverrides {
        cpu_millis: vm.cpu.map(|cores| cores * 1000),
        memory_bytes: match &vm.memory {
            Some(s) => match gita_pipeline::VmJob::parse_size(s) {
                Ok(b) => Some(b),
                Err(e) => {
                    let msg = format!("invalid vm memory size {s:?}: {e}");
                    tracing::warn!(job = job_id, "{msg}");
                    write_system_log(db, job_id, &msg).await?;
                    return Ok(VmJobOutcome::InfrastructureError);
                }
            },
            None => None,
        },
        disk_bytes: match &vm.disk {
            Some(s) => match gita_pipeline::VmJob::parse_size(s) {
                Ok(b) => Some(b),
                Err(e) => {
                    let msg = format!("invalid vm disk size {s:?}: {e}");
                    tracing::warn!(job = job_id, "{msg}");
                    write_system_log(db, job_id, &msg).await?;
                    return Ok(VmJobOutcome::InfrastructureError);
                }
            },
            None => None,
        },
        // Clone over HTTP: the repo URL is the server's own public
        // address. Private repos are a known limitation — see the
        // module docs.
        ..Default::default()
    };

    // 4. Compose the agent's user data. The job's environment variables
    //    (GITA_REPOSITORY etc.) are exported in the script preamble so
    //    VM jobs see the same surface local jobs do.
    let repo_label = repo_full_name(db, run).await;
    let script = format!(
        // The agent runs `bash <file>` without -e (its own -ec applies to
        // the outer shell only), so the file aborts on the first failing
        // command itself — the same semantics as the local executor's
        // `set -e`.
        "set -e\nexport GITA_REPOSITORY={repo}\nexport GITA_COMMIT_SHA={sha}\nexport GITA_REF={ref_}\nexport GITA_JOB_NAME={job_name}\nexport GITA_RUN_ID={run_id}\n{script}",
        repo = repo_label,
        sha = run.head_sha,
        ref_ = run.ref_name,
        job_name = job_name_for(db, job_id).await,
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
    let overrides = VmSpecOverrides {
        user_data: Some(userdata),
        ..overrides
    };

    // 5. Provision the VM. The expiry gives the control-plane GC a
    //    backstop: if this runner dies, the VM is still reclaimed.
    let timeout = config.job_timeout().unwrap_or(Duration::from_secs(3600));
    let helper = gita_vm::ci::CiVmHelper::new(store.clone());
    let opts = CreateVmOptions {
        owner: None,
        repo_id: Some(run.repo_id),
        pipeline_run_id: Some(run.id),
        job_id: Some(job_id),
        expires_at_ms: Some(gita_vm::now_ms() + timeout.as_millis() as i64 + 600_000),
        template_id: Some(template.id),
        overrides: Some(overrides),
    };
    let vm_result = match helper.create_ci_vm(&opts).await {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("failed to provision VM: {e}");
            tracing::warn!(job = job_id, "{msg}");
            write_system_log(db, job_id, &msg).await?;
            return Ok(VmJobOutcome::InfrastructureError);
        }
    };
    tracing::info!(job = job_id, vm = %vm_result.vm_id, "provisioned VM for job");

    // 6. Watch the job row until it settles. The agent moves it
    //    queued → running → success/failed; a user cancel also lands
    //    here.
    let outcome = watch_job(db, job_id, timeout).await;

    // 7. Reclaim the VM, whatever happened.
    if let Err(e) = helper.destroy_vm(vm_result.vm_id).await {
        tracing::warn!(job = job_id, vm = %vm_result.vm_id, "reclaim VM failed: {e}");
    }

    Ok(outcome)
}

/// Poll the job row until a terminal state or the timeout.
async fn watch_job(db: &Db, job_id: i64, timeout: Duration) -> VmJobOutcome {
    let mut ticker = interval(WATCH_POLL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        ticker.tick().await;

        match db.get_job(job_id).await {
            Ok(Some(job)) => match job.status.as_str() {
                "success" => return VmJobOutcome::Success,
                "failed" => return VmJobOutcome::Failed,
                "canceled" => return VmJobOutcome::Canceled,
                _ => {}
            },
            Ok(None) => {
                // The job row vanished — the run was deleted out from
                // under us. Nothing to wait for.
                tracing::warn!(job = job_id, "job row disappeared while watching");
                return VmJobOutcome::Failed;
            }
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
async fn write_system_log(db: &Db, job_id: i64, line: &str) -> Result<()> {
    let _ = db
        .insert_log(
            job_id,
            0,
            "system",
            line,
            &chrono::Utc::now().to_rfc3339(),
        )
        .await;
    Ok(())
}

async fn repo_full_name(db: &Db, run: &gita_db::PipelineRunRow) -> String {
    match db.get_repository(run.repo_id).await {
        Ok(Some(repo)) => repo.full_name(),
        _ => String::new(),
    }
}

async fn job_name_for(db: &Db, job_id: i64) -> String {
    db.get_job(job_id)
        .await
        .ok()
        .flatten()
        .map(|j| j.name)
        .unwrap_or_default()
}

// Re-export for the executor's use.
pub use gita_vm::now_ms as _vm_now_ms;

// Keep the unused-import lint honest: VmResult is part of the public
// signature surface only via inference today.
#[allow(unused)]
fn _assert_send<T: Send>(_t: T) {}
#[allow(dead_code)]
fn _unused_vm_result(_: VmResult<()>) {}
