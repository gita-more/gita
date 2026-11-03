//! Integration tests for the VM agent callback endpoints.
//!
//! A `vm:` job reports logs and status from inside a cluster VM using a
//! per-job token. These tests exercise the callback contract without any
//! actual virtual machine: the runner-side token minting, and the
//! agent-side endpoints, are the parts that can break in isolation.

mod common;

use axum::http::{Method, StatusCode};
use tower::ServiceExt;

/// Mint an agent token for `job_id` the way the runner does, returning
/// the plaintext. The database stores only the hash.
async fn mint_agent_token(app: &gita_admin::App, job_id: i64) -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    app.db
        .set_job_agent_token(job_id, &gita_db::hash_token(&token))
        .await
        .unwrap();
    token
}

/// Create a pipeline run + job directly in the database (the runner does
/// this too when it expands a pipeline).
async fn make_job(app: &gita_admin::App) -> (i64, i64) {
    // A repo to anchor the run.
    let user = common::register_user(&app, "agentowner").await;
    let full = common::create_repo(&app, &user.token, "agentrepo").await;
    let (owner, repo) = full.split_once('/').unwrap();
    let repo_row = app
        .db
        .get_repository_by_name(&owner, &repo)
        .await
        .unwrap()
        .unwrap();

    // A pipeline + run row.
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
    (run_id, job_id)
}

/// Without a token the callback endpoints reject the call.
#[tokio::test]
async fn agent_endpoints_reject_missing_token() {
    let (app, _guard) = common::test_app().await;
    let (_run_id, job_id) = make_job(&app).await;

    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri(format!("/api/ci/agent/jobs/{job_id}/status"))
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({"status": "running"})).unwrap(),
        ))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// A wrong token is the same as no token.
#[tokio::test]
async fn agent_endpoints_reject_wrong_token() {
    let (app, _guard) = common::test_app().await;
    let (_run_id, job_id) = make_job(&app).await;
    let _token = mint_agent_token(&app, job_id).await;

    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri(format!("/api/ci/agent/jobs/{job_id}/status"))
        .header("content-type", "application/json")
        .header("authorization", "Bearer wrong-token")
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({"status": "running"})).unwrap(),
        ))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// A job that never got an agent token (a locally executed job) must
/// reject callbacks even for a well-formed bearer — the token column is
/// NULL and nothing matches NULL.
#[tokio::test]
async fn locally_executed_job_rejects_callbacks() {
    let (app, _guard) = common::test_app().await;
    let (_run_id, job_id) = make_job(&app).await;
    // No token minted.

    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri(format!("/api/ci/agent/jobs/{job_id}/status"))
        .header("content-type", "application/json")
        .header("authorization", "Bearer anything")
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({"status": "running"})).unwrap(),
        ))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// The happy path: queued → running → success, then the logs are visible
/// through the regular job logs endpoint.
#[tokio::test]
async fn agent_can_run_a_job_to_completion() {
    let (app, _guard) = common::test_app().await;
    let (run_id, job_id) = make_job(&app).await;
    let token = mint_agent_token(&app, job_id).await;

    let send = |uri: String, body: serde_json::Value| {
        let app = &app;
        let token = &token;
        async move {
            let req = axum::http::Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap();
            app.router().oneshot(req).await.unwrap()
        }
    };

    // queued → running
    let resp = send(
        format!("/api/ci/agent/jobs/{job_id}/status"),
        serde_json::json!({"status": "running"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["applied"], true);

    // Log lines.
    let resp = send(
        format!("/api/ci/agent/jobs/{job_id}/logs"),
        serde_json::json!({"seq": 0, "stream": "stdout", "line": "compiling..."}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = send(
        format!("/api/ci/agent/jobs/{job_id}/logs"),
        serde_json::json!({"seq": 1, "stream": "stderr", "line": "a warning"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // running → success
    let resp = send(
        format!("/api/ci/agent/jobs/{job_id}/status"),
        serde_json::json!({"status": "success", "exit_code": 0}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // The job row reflects the transition.
    let job = app.db.get_job(job_id).await.unwrap().unwrap();
    assert_eq!(job.status, "success");
    assert_eq!(job.exit_code, Some(0));
    assert!(job.started_at.is_some());
    assert!(job.finished_at.is_some());

    // And the logs the agent wrote are in the table.
    let logs = app.db.list_logs_since(job_id, 0, 100).await.unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0].stream, "stdout");
    assert_eq!(logs[1].stream, "stderr");
    assert_eq!(logs[0].line, "compiling...");
    let _ = run_id;
}

/// A replayed or racing callback cannot move the job again: once
/// finished, further status reports are no-ops (200 with applied=false).
#[tokio::test]
async fn finished_job_ignores_further_status_reports() {
    let (app, _guard) = common::test_app().await;
    let (_run_id, job_id) = make_job(&app).await;
    let token = mint_agent_token(&app, job_id).await;

    let send = |body: serde_json::Value| {
        let app = &app;
        let token = &token;
        async move {
            let req = axum::http::Request::builder()
                .method(Method::POST)
                .uri(format!("/api/ci/agent/jobs/{job_id}/status"))
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap();
            app.router().oneshot(req).await.unwrap()
        }
    };

    send(serde_json::json!({"status": "running"})).await;
    send(serde_json::json!({"status": "success", "exit_code": 0})).await;

    // A second "success" (a retried callback) must not change the row.
    let resp = send(serde_json::json!({"status": "success", "exit_code": 0})).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["applied"], false);

    // And a "failed" report after success must not resurrect the job.
    send(serde_json::json!({"status": "failed", "exit_code": 1})).await;
    let job = app.db.get_job(job_id).await.unwrap().unwrap();
    assert_eq!(job.status, "success");
}

/// An agent cannot skip states: `success` straight from `queued` is a
/// no-op, because the job never ran.
#[tokio::test]
async fn agent_cannot_finish_a_job_that_never_started() {
    let (app, _guard) = common::test_app().await;
    let (_run_id, job_id) = make_job(&app).await;
    let token = mint_agent_token(&app, job_id).await;

    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri(format!("/api/ci/agent/jobs/{job_id}/status"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({"status": "success", "exit_code": 0})).unwrap(),
        ))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["applied"], false);

    let job = app.db.get_job(job_id).await.unwrap().unwrap();
    assert_eq!(job.status, "queued", "the job must stay queued");
}

/// An agent may only report the three states it owns; canceled comes from
/// the user side.
#[tokio::test]
async fn agent_cannot_report_canceled() {
    let (app, _guard) = common::test_app().await;
    let (_run_id, job_id) = make_job(&app).await;
    let token = mint_agent_token(&app, job_id).await;

    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri(format!("/api/ci/agent/jobs/{job_id}/status"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({"status": "canceled"})).unwrap(),
        ))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Log lines must use the two stream names the schema allows.
#[tokio::test]
async fn agent_log_stream_is_validated() {
    let (app, _guard) = common::test_app().await;
    let (_run_id, job_id) = make_job(&app).await;
    let token = mint_agent_token(&app, job_id).await;

    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri(format!("/api/ci/agent/jobs/{job_id}/logs"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({
                "seq": 0, "stream": "system", "line": "bogus"
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp = app.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
