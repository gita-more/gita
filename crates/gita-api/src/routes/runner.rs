//! CI runner API.
//!
//! The runner used to talk to Postgres directly: it claimed runs with a
//! `FOR UPDATE SKIP LOCKED` query of its own, wrote job rows, log lines
//! and status updates straight into the tables, and even handed its
//! connection pool to the VM store. That tied every runner to a database
//! credential and to the schema — a runner could only run where the
//! database was reachable, and it could do anything the database allowed.
//!
//! These endpoints are the server-side half of the fix: the runner now
//! asks the server to do the database work, and the server stays the only
//! component that knows the schema.
//!
//! ## Authentication
//!
//! A **runner token**: a personal access token whose stored name starts
//! with `runner/` (that is what `gita-admin worker register` mints). The
//! name check is what keeps an ordinary user PAT from claiming CI work
//! and writing job logs — the token is scoped to its kind, not just to
//! its owner.
//!
//! ## The claim endpoint
//!
//! `POST /api/runner/claim` is the only place a run is claimed. The
//! atomicity comes from the database, not from the runner: the server
//! runs the same `FOR UPDATE SKIP LOCKED` statement it always did, so
//! several runners polling at once still hand each run to exactly one of
//! them.

use crate::error::{ApiError, ApiResult};
use crate::extract::AuthUserExtractor;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use gita_db::{JobRow, PipelineRow, PipelineRunRow};
use serde::{Deserialize, Serialize};

pub fn runner_routes() -> Router<AppState> {
    Router::new()
        .route("/api/runner/claim", post(claim_run))
        .route("/api/runner/runs/{run_id}", get(get_run))
        .route("/api/runner/runs/{run_id}/status", post(set_run_status))
        .route("/api/runner/runs/{run_id}/renew", post(renew_run))
        .route("/api/runner/runs/{run_id}/jobs", post(create_job))
        .route("/api/runner/jobs/{job_id}", get(get_job))
        .route("/api/runner/jobs/{job_id}/status", post(set_job_status))
        .route("/api/runner/jobs/{job_id}/logs", post(append_logs))
        .route("/api/runner/repos/{repo_id}", get(get_repo))
}

/// Reject anything that is not a runner token.
///
/// The token itself is checked by the extractor (a valid PAT of any
/// kind); this adds the *kind* check.
async fn require_runner_token(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::Unauthorized("missing runner token".into()))?;

    let name = state
        .db
        .get_token_name(token)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("invalid runner token".into()))?;
    if !name.starts_with("runner/") {
        return Err(ApiError::Forbidden(
            "this endpoint requires a runner token (one named runner/…)".into(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Claim
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ClaimRequest {
    /// The name the runner reports itself as; recorded on the run row.
    pub runner_name: String,
    /// How long the claim is held before another runner may take the run.
    #[serde(default = "default_lease")]
    pub lease_secs: i64,
}

fn default_lease() -> i64 {
    300
}

#[derive(Debug, Serialize)]
pub struct ClaimResponse {
    /// The claimed run, or `None` when there is nothing to do.
    pub run: Option<PipelineRunRow>,
    /// The pipeline the run executes.
    ///
    /// Its `path` decides whether the run is a `.gita-ci.yml` pipeline or a
    /// GitHub-style workflow, and its `content` is the YAML itself — the
    /// runner needs both, and the runner has no database to ask.
    pub pipeline: Option<PipelineRow>,
}

/// POST /api/runner/claim
async fn claim_run(
    State(state): State<AppState>,
    _user: AuthUserExtractor,
    headers: HeaderMap,
    Json(req): Json<ClaimRequest>,
) -> ApiResult<Json<ClaimResponse>> {
    require_runner_token(&state, &headers).await?;

    let run = state
        .db
        .claim_pipeline_run(&req.runner_name, req.lease_secs)
        .await?;
    let pipeline = match &run {
        Some(run) => state.db.get_pipeline(run.pipeline_id).await?,
        // Nothing was claimed, so there is nothing to look up.
        None => None,
    };
    Ok(Json(ClaimResponse { run, pipeline }))
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

/// GET /api/runner/runs/{run_id}
async fn get_run(
    State(state): State<AppState>,
    _user: AuthUserExtractor,
    headers: HeaderMap,
    Path(run_id): Path<i64>,
) -> ApiResult<Json<PipelineRunRow>> {
    require_runner_token(&state, &headers).await?;
    let run = state
        .db
        .get_pipeline_run(run_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("run not found".into()))?;
    Ok(Json(run))
}

#[derive(Debug, Deserialize)]
pub struct RunStatusRequest {
    pub status: String,
}

/// POST /api/runner/runs/{run_id}/status
async fn set_run_status(
    State(state): State<AppState>,
    _user: AuthUserExtractor,
    headers: HeaderMap,
    Path(run_id): Path<i64>,
    Json(req): Json<RunStatusRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    require_runner_token(&state, &headers).await?;
    if !matches!(
        req.status.as_str(),
        "running" | "success" | "failed" | "canceled"
    ) {
        return Err(ApiError::BadRequest(format!(
            "unknown run status {:?}",
            req.status
        )));
    }
    state
        .db
        .update_pipeline_run_status(run_id, &req.status)
        .await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

#[derive(Debug, Deserialize)]
pub struct RenewRequest {
    /// The runner that holds the lease; the renew only succeeds from it.
    pub runner_name: String,
    #[serde(default = "default_lease")]
    pub lease_secs: i64,
}

/// POST /api/runner/runs/{run_id}/renew
///
/// Extend the lease on a run the runner is executing. A `false` answer
/// means the run moved on without this runner (finished, canceled, or
/// reaped), and it should stop working on it.
async fn renew_run(
    State(state): State<AppState>,
    _user: AuthUserExtractor,
    headers: HeaderMap,
    Path(run_id): Path<i64>,
    Json(req): Json<RenewRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    require_runner_token(&state, &headers).await?;
    let renewed = state
        .db
        .renew_pipeline_run(run_id, &req.runner_name, req.lease_secs)
        .await?;
    Ok(Json(serde_json::json!({ "renewed": renewed })))
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------
#[derive(Debug, Deserialize)]
pub struct CreateJobRequest {
    pub name: String,
    pub stage: String,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub script: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateJobResponse {
    pub job_id: i64,
}

/// POST /api/runner/runs/{run_id}/jobs
async fn create_job(
    State(state): State<AppState>,
    _user: AuthUserExtractor,
    headers: HeaderMap,
    Path(run_id): Path<i64>,
    Json(req): Json<CreateJobRequest>,
) -> ApiResult<Json<CreateJobResponse>> {
    require_runner_token(&state, &headers).await?;
    let job_id = state
        .db
        .insert_job(run_id, &req.name, &req.stage, &req.needs, &req.script)
        .await?;
    Ok(Json(CreateJobResponse { job_id }))
}

/// GET /api/runner/jobs/{job_id}
async fn get_job(
    State(state): State<AppState>,
    _user: AuthUserExtractor,
    headers: HeaderMap,
    Path(job_id): Path<i64>,
) -> ApiResult<Json<JobRow>> {
    require_runner_token(&state, &headers).await?;
    let job = state
        .db
        .get_job(job_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("job not found".into()))?;
    Ok(Json(job))
}

#[derive(Debug, Deserialize)]
pub struct JobStatusRequest {
    pub status: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

/// POST /api/runner/jobs/{job_id}/status
async fn set_job_status(
    State(state): State<AppState>,
    _user: AuthUserExtractor,
    headers: HeaderMap,
    Path(job_id): Path<i64>,
    Json(req): Json<JobStatusRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    require_runner_token(&state, &headers).await?;
    if !matches!(
        req.status.as_str(),
        "queued" | "running" | "success" | "failed" | "canceled"
    ) {
        return Err(ApiError::BadRequest(format!(
            "unknown job status {:?}",
            req.status
        )));
    }
    state
        .db
        .update_job_status(job_id, &req.status, req.exit_code)
        .await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct LogLine {
    pub seq: i64,
    /// `"stdout"`, `"stderr"` or `"system"`.
    pub stream: String,
    pub line: String,
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// POST /api/runner/jobs/{job_id}/logs
///
/// Logs arrive in batches: a chatty build would otherwise cost one HTTP
/// request per line. The insert is idempotent per `(job_id, seq)`, so a
/// retried batch does not duplicate anything.
async fn append_logs(
    State(state): State<AppState>,
    _user: AuthUserExtractor,
    headers: HeaderMap,
    Path(job_id): Path<i64>,
    Json(lines): Json<Vec<LogLine>>,
) -> ApiResult<Json<serde_json::Value>> {
    require_runner_token(&state, &headers).await?;
    for entry in lines {
        // The `logs` table allows these three; `system` carries the
        // runner's own notes (a failed provisioning, a missing template).
        if !matches!(entry.stream.as_str(), "stdout" | "stderr" | "system") {
            return Err(ApiError::BadRequest(format!(
                "stream must be stdout, stderr or system, got {:?}",
                entry.stream
            )));
        }
        let ts = entry
            .timestamp
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
        state
            .db
            .insert_log(job_id, entry.seq, &entry.stream, &entry.line, &ts)
            .await?;
    }
    Ok(Json(serde_json::json!({"ok": true})))
}

// ---------------------------------------------------------------------------
// Repositories
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct RepoRef {
    pub owner_name: String,
    pub name: String,
}

/// GET /api/runner/repos/{repo_id}
async fn get_repo(
    State(state): State<AppState>,
    _user: AuthUserExtractor,
    headers: HeaderMap,
    Path(repo_id): Path<i64>,
) -> ApiResult<Json<RepoRef>> {
    require_runner_token(&state, &headers).await?;
    let repo = state
        .db
        .get_repository(repo_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    Ok(Json(RepoRef {
        owner_name: repo.owner_name,
        name: repo.name,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------------
    // default_lease
    // ---------------------------------------------------------------------------

    #[test]
    fn default_lease_is_five_minutes() {
        assert_eq!(default_lease(), 300);
    }

    // ---------------------------------------------------------------------------
    // ClaimRequest validation
    // ---------------------------------------------------------------------------

    #[test]
    fn claim_request_lease_secs_defaults_to_300() {
        let req: ClaimRequest = serde_json::from_str(r#"{"runner_name":"r1"}"#).unwrap();
        assert_eq!(req.runner_name, "r1");
        assert_eq!(req.lease_secs, 300);
    }

    #[test]
    fn claim_request_lease_secs_from_body() {
        let req: ClaimRequest =
            serde_json::from_str(r#"{"runner_name":"r1","lease_secs":600}"#).unwrap();
        assert_eq!(req.runner_name, "r1");
        assert_eq!(req.lease_secs, 600);
    }

    // ---------------------------------------------------------------------------
    // CreateJobRequest validation
    // ---------------------------------------------------------------------------

    #[test]
    fn create_job_request_defaults() {
        let req: CreateJobRequest = serde_json::from_str(r#"{"name":"build","stage":"build"}"#).unwrap();
        assert_eq!(req.name, "build");
        assert_eq!(req.stage, "build");
        assert_eq!(req.needs, Vec::<String>::new());
        assert_eq!(req.script, Vec::<String>::new());
    }

    #[test]
    fn create_job_request_with_values() {
        let req: CreateJobRequest =
            serde_json::from_str(r#"{"name":"build","stage":"build","needs":["test"],"script":["echo hi"]}"#)
                .unwrap();
        assert_eq!(req.name, "build");
        assert_eq!(req.stage, "build");
        assert_eq!(req.needs, vec!["test"]);
        assert_eq!(req.script, vec!["echo hi"]);
    }

    // ---------------------------------------------------------------------------
    // JobStatusRequest validation
    // ---------------------------------------------------------------------------

    #[test]
    fn job_status_request_accepts_valid_statuses() {
        for status in ["queued", "running", "success", "failed", "canceled"] {
            let req: JobStatusRequest =
                serde_json::from_str(&format!(r#"{{"status":"{status}"}}"#)).unwrap();
            assert_eq!(req.status, status);
            assert_eq!(req.exit_code, None);
        }
    }

    #[test]
    fn job_status_request_with_exit_code() {
        let req: JobStatusRequest =
            serde_json::from_str(r#"{"status":"failed","exit_code":1}"#).unwrap();
        assert_eq!(req.status, "failed");
        assert_eq!(req.exit_code, Some(1));
    }

    // ---------------------------------------------------------------------------
    // RunStatusRequest validation
    // ---------------------------------------------------------------------------

    #[test]
    fn run_status_request_accepts_valid_statuses() {
        for status in ["running", "success", "failed", "canceled"] {
            let req: RunStatusRequest =
                serde_json::from_str(&format!(r#"{{"status":"{status}"}}"#)).unwrap();
            assert_eq!(req.status, status);
        }
    }

    // ---------------------------------------------------------------------------
    // LogLine validation
    // ---------------------------------------------------------------------------

    #[test]
    fn log_line_defaults_timestamp() {
        let line: LogLine =
            serde_json::from_str(r#"{"seq":1,"stream":"stdout","line":"hi"}"#).unwrap();
        assert_eq!(line.seq, 1);
        assert_eq!(line.stream, "stdout");
        assert_eq!(line.line, "hi");
        assert!(line.timestamp.is_none());
    }

    #[test]
    fn log_line_accepts_timestamp() {
        let line: LogLine = serde_json::from_str(
            r#"{"seq":1,"stream":"stderr","line":"err","timestamp":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(line.stream, "stderr");
        assert!(line.timestamp.is_some());
    }

    // ---------------------------------------------------------------------------
    // Log stream validation
    // ---------------------------------------------------------------------------

    #[test]
    fn log_stream_accepts_stdout_stderr_system() {
        for stream in ["stdout", "stderr", "system"] {
            let line: LogLine =
                serde_json::from_str(&format!(r#"{{"seq":1,"stream":"{stream}","line":"x"}}"#)).unwrap();
            assert_eq!(line.stream, stream);
        }
    }

    #[test]
    fn log_stream_parses_any_stream_string_validation_happens_at_handler() {
        // The LogLine struct itself accepts any stream string; the
        // "stream must be stdout/stderr/system" check is enforced by the
        // append_logs handler, not at deserialization.
        let line: LogLine =
            serde_json::from_str(r#"{"seq":1,"stream":"bogus","line":"x"}"#).unwrap();
        assert_eq!(line.stream, "bogus");
    }
}
