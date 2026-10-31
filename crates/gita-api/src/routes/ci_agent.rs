//! VM agent callback endpoints.
//!
//! A job declared with a `vm:` block runs inside a cluster virtual
//! machine: the runner provisions the VM and injects the job's execution
//! material through cloud-init. The agent inside the VM reports progress
//! back through the two endpoints here.
//!
//! Authentication is a **per-job token**, not a user credential. The
//! runner mints a random token when it provisions the VM, stores only its
//! SHA-256 hash on the job row, and injects the plaintext into the VM's
//! user data. Consequences:
//!
//! - a leaked token is scoped to exactly one job;
//! - a token is dead the moment the job finishes;
//! - jobs that never had a token (locally executed ones) are rejected —
//!   no amount of guessing grants write access to them.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;

pub fn ci_agent_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/ci/agent/jobs/{job_id}/logs",
            post(agent_append_log),
        )
        .route(
            "/api/ci/agent/jobs/{job_id}/status",
            post(agent_report_status),
        )
}

/// Extract and verify the per-job bearer token.
async fn require_job_token(
    state: &AppState,
    job_id: i64,
    headers: &axum::http::HeaderMap,
) -> ApiResult<()> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::Unauthorized("missing agent token".into()))?;

    if !state.db.verify_job_agent_token(job_id, token).await? {
        // A wrong token and a nonexistent job are the same answer, so a
        // caller cannot probe which job ids exist.
        return Err(ApiError::Unauthorized("invalid agent token".into()));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct AgentLogRequest {
    pub seq: i64,
    /// `"stdout"` or `"stderr"` — the `logs` table checks this.
    pub stream: String,
    pub line: String,
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// POST /api/ci/agent/jobs/{job_id}/logs
async fn agent_append_log(
    State(state): State<AppState>,
    Path(job_id): Path<i64>,
    headers: axum::http::HeaderMap,
    Json(req): Json<AgentLogRequest>,
) -> ApiResult<axum::response::Response> {
    require_job_token(&state, job_id, &headers).await?;

    // Only the two stream names the schema allows; anything else is a
    // client bug, not a server error.
    if req.stream != "stdout" && req.stream != "stderr" {
        return Err(ApiError::BadRequest(format!(
            "stream must be stdout or stderr, got {:?}",
            req.stream
        )));
    }

    let ts = req
        .timestamp
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    state.db.insert_log(job_id, req.seq, &req.stream, &req.line, &ts).await?;
    Ok((axum::http::StatusCode::OK, axum::Json(serde_json::json!({"ok": true}))).into_response())
}

#[derive(Debug, Deserialize)]
pub struct AgentStatusRequest {
    pub status: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

/// POST /api/ci/agent/jobs/{job_id}/status
///
/// The transition rules live in [`gita_db::Db::agent_update_job_status`]:
/// queued → running → success/failed only. A replayed callback cannot
/// resurrect a finished job, and a finishing agent cannot clobber a
/// cancel.
async fn agent_report_status(
    State(state): State<AppState>,
    Path(job_id): Path<i64>,
    headers: axum::http::HeaderMap,
    Json(req): Json<AgentStatusRequest>,
) -> ApiResult<axum::response::Response> {
    require_job_token(&state, job_id, &headers).await?;

    if !matches!(req.status.as_str(), "running" | "success" | "failed") {
        return Err(ApiError::BadRequest(format!(
            "agent may only report running/success/failed, got {:?}",
            req.status
        )));
    }

    let applied = state
        .db
        .agent_update_job_status(job_id, &req.status, req.exit_code)
        .await?;
    if !applied {
        // Not an error: the job has moved on (canceled, or already
        // finished by a previous callback). Answer OK so the agent does
        // not retry forever.
        return Ok((
            axum::http::StatusCode::OK,
            axum::Json(serde_json::json!({"ok": true, "applied": false})),
        )
            .into_response());
    }
    Ok((
        axum::http::StatusCode::OK,
        axum::Json(serde_json::json!({"ok": true, "applied": true})),
    )
        .into_response())
}
