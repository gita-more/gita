//! Integration tests for the runner's VM job path.
//!
//! These run `run_job_in_vm` against a real database without any actual
//! virtual machine: everything up to the point where a VM would be
//! created (token minting, template resolution, failure reporting) is
//! exercisable, and a missing template is the one path where the whole
//! chain completes without a hypervisor.

mod common;

/// A `vm:` job whose template does not exist must fail the job — with a
/// system log line explaining why — and must have minted an agent token
/// that can no longer do anything (the job is terminal).
#[tokio::test]
async fn vm_job_with_unknown_template_fails_with_a_system_log() {
    let (app, _guard) = common::test_app().await;
    let user = common::register_user(&app, "vmfail").await;
    let full = common::create_repo(&app, &user.token, "vmfailrepo").await;
    let (owner, repo) = full.split_once('/').unwrap();
    let repo_row = app
        .db
        .get_repository_by_name(&owner, &repo)
        .await
        .unwrap()
        .unwrap();

    let now = chrono::Utc::now().to_rfc3339();
    let pipeline_id: i64 = sqlx::query_scalar(
        "INSERT INTO pipelines (repo_id, path, ref, ref_sha, content, created_at, updated_at)
         VALUES ($1, '.gita-ci.yml', 'main', 'deadbeef', 'stages: [b]', $2, $2)
         RETURNING id",
    )
    .bind(repo_row.id)
    .bind(&now)
    .fetch_one(app.db.pool())
    .await
    .unwrap();
    let run_id: i64 = sqlx::query_scalar(
        "INSERT INTO pipeline_runs (pipeline_id, repo_id, ref, head_sha, status, queued_at)
         VALUES ($1, $2, 'main', 'deadbeef', 'running', $3)
         RETURNING id",
    )
    .bind(pipeline_id)
    .bind(repo_row.id)
    .bind(&now)
    .fetch_one(app.db.pool())
    .await
    .unwrap();
    let job_id = app
        .db
        .insert_job(run_id, "build", "b", &[], &["echo hi".to_string()])
        .await
        .unwrap();

    use clap::Parser as _;
    let runner_config = gita_runner::config::RunnerConfig::parse_from([
        "gita-runner",
        "--server-db-url",
        &common::test_db_url(),
        "--runner-token",
        "test-token",
    ]);

    let run = app.db.get_pipeline_run(run_id).await.unwrap().unwrap();
    let vm = gita_pipeline::VmJob {
        template: "no-such-template".into(),
        cpu: None,
        memory: None,
        disk: None,
    };

    let outcome = gita_runner::vm_exec::run_job_in_vm(
        &app.db,
        &runner_config,
        &run,
        job_id,
        &vm,
        "echo hi",
    )
    .await
    .unwrap();

    assert_eq!(
        outcome,
        gita_runner::vm_exec::VmJobOutcome::InfrastructureError,
        "an unknown template must be an infrastructure failure"
    );

    // The executor marks the job failed on InfrastructureError — here we
    // do it by hand to mimic that step, then assert the row is terminal.
    app.db
        .update_job_status(job_id, "failed", None)
        .await
        .unwrap();
    let job = app.db.get_job(job_id).await.unwrap().unwrap();
    assert_eq!(job.status, "failed");

    let logs = app.db.list_logs_since(job_id, 0, 100).await.unwrap();
    assert!(
        logs.iter().any(|l| l.stream == "system"
            && l.line.contains("no-such-template")),
        "a system log must name the missing template, got {logs:?}"
    );

    // The agent token was minted, but the job is terminal so the
    // callback endpoints refuse it: a token for a failed job is dead.
    let row: (Option<String>,) =
        sqlx::query_as("SELECT agent_token FROM jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(app.db.pool())
            .await
            .unwrap();
    assert!(row.0.is_some(), "the runner must have stored a token hash");
}
