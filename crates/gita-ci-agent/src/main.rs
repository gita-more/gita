//! Gita CI Agent — one-shot job execution inside a CI VM.
//!
//! The runner provisions a VM for a `vm:` job and injects everything the
//! agent needs through cloud-init user data: which repository, which
//! commit, which command, and the per-job callback token. There is
//! nothing to poll: the VM boots with exactly one job to run, and the
//! agent's whole lifecycle is
//!
//! ```text
//! report running → clone → checkout → run script → report success/failed
//! ```
//!
//! (An earlier iteration polled `/api/v1/ci/agents/{id}/jobs/next`, but
//! no server ever implemented those endpoints — the polling loop was dead
//! code. One VM per job also gives the cleaner blast radius: a job's
//! environment is always fresh, and reclaiming the VM reclaims
//! everything about the job.)
//!
//! The behaviour lives in the library; this binary is the wiring.

use clap::Parser;
use gita_ci_agent::{
    checkout_command, clone_command, report_status, run_command, script_command, write_ssh_key,
    Args, LogSink,
};
use std::path::Path;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    info!(
        job = args.job_id,
        server = %args.server_url,
        "gita-ci-agent starting for one job"
    );

    let sink = LogSink::new(&args.server_url, &args.token);

    // Claim the job first: queued → running. If the report fails, keep
    // going anyway — the runner's timeout is the safety net.
    report_status(&args, "running", None).await;

    // Prepare the workspace.
    let work_dir = format!("/home/ci/work/{}", args.job_id);
    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        sink.write(args.job_id, "stderr", format!("create {work_dir}: {e}"));
        report_status(&args, "failed", None).await;
        anyhow::bail!("cannot create work dir: {e}");
    }

    // Private repos clone over SSH; write the key with tight permissions
    // before any git command runs.
    let ssh_key_path = Path::new("/home/ci/.ssh/id_ed25519");
    if !args.ssh_key.is_empty() {
        if let Err(e) = write_ssh_key(ssh_key_path, &args.ssh_key) {
            sink.write(args.job_id, "stderr", format!("write ssh key: {e}"));
        }
    }

    // Clone + checkout the commit the run names.
    let clone = clone_command(&args.repo_url, Path::new(&work_dir), ssh_key_path);
    let Some(status) = run_command(&args, &sink, clone, "git clone").await else {
        report_status(&args, "failed", None).await;
        anyhow::bail!("git clone could not run");
    };
    if !status.success() {
        report_status(&args, "failed", status.code()).await;
        anyhow::bail!("git clone failed");
    }

    let checkout = checkout_command(&args.commit, Path::new(&work_dir));
    let Some(status) = run_command(&args, &sink, checkout, "git checkout").await else {
        report_status(&args, "failed", None).await;
        anyhow::bail!("git checkout could not run");
    };
    if !status.success() {
        report_status(&args, "failed", status.code()).await;
        anyhow::bail!("git checkout failed");
    }

    // Start the job's service containers. This happens before the script
    // so a step can connect on the first try; the containers are torn down
    // whatever the script's outcome.
    let services: std::collections::BTreeMap<String, gita_workflow::Service> =
        match args.services_json().and_then(|j| {
            serde_json::from_str(&j).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        }) {
            Ok(s) => s,
            Err(e) => {
                sink.write(args.job_id, "stderr", format!("bad GITA_AGENT_SERVICES: {e}"));
                report_status(&args, "failed", None).await;
                anyhow::bail!("cannot parse services: {e}");
            }
        };
    let sink_for_services = sink.clone();
    let job_id = args.job_id;
    let log = move |line: &str| {
        sink_for_services.write(job_id, "system", line.to_string());
    };
    let mut running = match gita_ci_agent::services::Services::start(job_id, &services, &log).await {
        Ok(s) => s,
        Err(e) => {
            log(&format!("services failed to start: {e}"));
            report_status(&args, "failed", None).await;
            anyhow::bail!("services failed to start: {e}");
        }
    };

    // Run the job's script.
    let script_cmd = script_command(&args.script, Path::new(&work_dir));
    let status = run_command(&args, &sink, script_cmd, "script").await;

    running.stop().await;

    let Some(status) = status else {
        report_status(&args, "failed", None).await;
        anyhow::bail!("script shell could not run");
    };

    let code = status.code();
    if status.success() {
        info!(job = args.job_id, "job finished successfully");
        report_status(&args, "success", code).await;
        Ok(())
    } else {
        info!(job = args.job_id, ?code, "job failed");
        report_status(&args, "failed", code).await;
        // The exit code is the job's outcome, not an agent malfunction —
        // a non-zero script exit is a normal way for this process to end.
        std::process::exit(code.unwrap_or(1));
    }
}
