//! Pipeline routes: list pipeline configs, list runs, get a run, cancel
//! a run, and stream log lines. Phase 1 endpoints only — see goal.md
//! §In-repo CI for the deferred Phase 2/3 surface.

use crate::error::{ApiError, ApiResult};
use crate::extract::AuthUserExtractor;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use gita_db::{JobRow, LogRow, PipelineRow, PipelineRunRow};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;

// ----- Response shapes --------------------------------------------------

#[derive(Debug, Serialize)]
pub struct PipelineResponse {
    pub id: i64,
    pub repo_id: i64,
    pub path: String,
    pub r#ref: String,
    pub ref_sha: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<PipelineRow> for PipelineResponse {
    fn from(p: PipelineRow) -> Self {
        Self {
            id: p.id,
            repo_id: p.repo_id,
            path: p.path,
            r#ref: p.ref_name,
            ref_sha: p.ref_sha,
            content: p.content,
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PipelineRunResponse {
    pub id: i64,
    pub pipeline_id: i64,
    pub repo_id: i64,
    pub r#ref: String,
    pub head_sha: String,
    pub trigger_event_id: Option<i64>,
    pub trigger_pusher_id: Option<i64>,
    pub status: String,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

impl From<PipelineRunRow> for PipelineRunResponse {
    fn from(r: PipelineRunRow) -> Self {
        Self {
            id: r.id,
            pipeline_id: r.pipeline_id,
            repo_id: r.repo_id,
            r#ref: r.ref_name,
            head_sha: r.head_sha,
            trigger_event_id: r.trigger_event_id,
            trigger_pusher_id: r.trigger_pusher_id,
            status: r.status,
            queued_at: r.queued_at,
            started_at: r.started_at,
            finished_at: r.finished_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct JobResponse {
    pub id: i64,
    pub run_id: i64,
    pub name: String,
    pub stage: String,
    pub needs: Vec<String>,
    pub script: Vec<String>,
    pub status: String,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
}

impl From<JobRow> for JobResponse {
    fn from(j: JobRow) -> Self {
        Self {
            id: j.id,
            run_id: j.run_id,
            name: j.name,
            stage: j.stage,
            needs: j.needs,
            script: j.script,
            status: j.status,
            queued_at: j.queued_at,
            started_at: j.started_at,
            finished_at: j.finished_at,
            exit_code: j.exit_code,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LogLineResponse {
    pub seq: i64,
    pub stream: String,
    pub line: String,
    pub ts: String,
}

impl From<LogRow> for LogLineResponse {
    fn from(r: LogRow) -> Self {
        Self {
            seq: r.seq,
            stream: r.stream,
            line: r.line,
            ts: r.ts,
        }
    }
}

// ----- Handlers ---------------------------------------------------------

/// List pipeline configs for a repo. Phase 1 returns the latest
/// `.gita-ci.yml` snapshot from the default branch — at most one
/// entry. If the file doesn't exist at the default branch's HEAD, the
/// response is `{pipelines: []}` with HTTP 200.
async fn list_pipelines(
    State(state): State<AppState>,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Json<PipelinesResponse>> {
    // Find the repo + default branch.
    let repo_row = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let head = state
        .git
        .head_sha(&owner, &repo, &repo_row.default_branch)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let content = state
        .git
        // Argument order is (owner, name, reference, path) — the ref and
        // path were swapped here, so the lookup never resolved and the
        // pipeline list was always empty.
        .read_file(&owner, &repo, &head, ".gita-ci.yml")
        .ok()
        .flatten();
    let pipelines = match content {
        Some(file) => {
            let row = state
                .db
                .get_pipeline_by_path(repo_row.id, ".gita-ci.yml")
                .await?;
            // Always synthesize a synthetic record from the file content;
            // if a DB row exists, prefer its id/timestamps.
            let (id, created_at, updated_at) = row
                .map(|r| (r.id, r.created_at, r.updated_at))
                .unwrap_or((0, String::new(), String::new()));
            vec![PipelineResponse {
                id,
                repo_id: repo_row.id,
                path: ".gita-ci.yml".into(),
                r#ref: format!("refs/heads/{}", repo_row.default_branch),
                ref_sha: head,
                created_at,
                updated_at,
                content: file.content,
            }]
        }
        None => Vec::new(),
    };
    Ok(Json(PipelinesResponse { pipelines }))
}

async fn list_runs(
    State(state): State<AppState>,
    Path((owner, repo, path)): Path<(String, String, String)>,
) -> ApiResult<Json<RunsResponse>> {
    let repo_row = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    let pipeline = state
        .db
        .get_pipeline_by_path(repo_row.id, &path)
        .await?
        .ok_or_else(|| ApiError::NotFound("pipeline not found".into()))?;
    let rows = state.db.list_pipeline_runs(pipeline.id, 50).await?;
    Ok(Json(RunsResponse {
        runs: rows.into_iter().map(PipelineRunResponse::from).collect(),
    }))
}

async fn get_run(
    State(state): State<AppState>,
    Path((_owner, _repo, run_id)): Path<(String, String, i64)>,
) -> ApiResult<Json<RunDetailResponse>> {
    let run = state
        .db
        .get_pipeline_run(run_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("run not found".into()))?;
    let jobs = state.db.list_jobs(run_id).await?;
    Ok(Json(RunDetailResponse {
        run: PipelineRunResponse::from(run),
        jobs: jobs.into_iter().map(JobResponse::from).collect(),
    }))
}

/// Manually trigger a new pipeline run from the current `.gita-ci.yml`
/// on the pipeline's stored ref + SHA. Auth required; only the
/// repository owner may re-run.
async fn trigger_run(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path((owner, repo, path)): Path<(String, String, String)>,
) -> ApiResult<Json<RunTriggerResponse>> {
    let repo = state
        .db
        .get_repository_by_name(&owner, &repo)
        .await?
        .ok_or_else(|| ApiError::NotFound("repository not found".into()))?;
    if repo.owner_id != user.id {
        return Err(ApiError::Forbidden(
            "only the repository owner may trigger a run".into(),
        ));
    }
    let pipeline = state
        .db
        .get_pipeline_by_path(repo.id, &path)
        .await?
        .ok_or_else(|| ApiError::NotFound("pipeline not found".into()))?;

    // Re-resolve head sha at the pipeline's ref so the run reflects the
    // current tip, not necessarily the SHA that last enqueued a run.
    let head_sha = state
        .git
        .head_sha(&owner, &repo.name, &pipeline.ref_name)
        .map_err(ApiError::from)?;

    let run_id = state
        .db
        .insert_pipeline_run(
            pipeline.id,
            repo.id,
            &pipeline.ref_name,
            &head_sha,
            Some(user.id),
            None,
        )
        .await?;

    // Mirror what the push handler does so the rest of the system
    // observes a PipelineRequested event for the new run.
    state.bus.publish(
        gita_core::EventType::PipelineRequested,
        Some(repo.id),
        Some(user.id),
        serde_json::json!({
            "owner": owner,
            "repo": repo.name,
            "run_id": run_id,
            "pipeline_id": pipeline.id,
            "ref": pipeline.ref_name,
            "head_sha": head_sha,
            "trigger": "manual",
        }),
    );

    let run = state
        .db
        .get_pipeline_run(run_id)
        .await?
        .ok_or_else(|| ApiError::Internal("inserted run vanished".into()))?;
    Ok(Json(RunTriggerResponse {
        run: PipelineRunResponse::from(run),
    }))
}

#[derive(Debug, Serialize)]
pub struct RunTriggerResponse {
    pub run: PipelineRunResponse,
}

async fn cancel_run(
    State(state): State<AppState>,
    _user: AuthUserExtractor,
    Path((_owner, _repo, run_id)): Path<(String, String, i64)>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let run = state
        .db
        .get_pipeline_run(run_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("run not found".into()))?;
    if !matches!(run.status.as_str(), "queued" | "running") {
        return Err(ApiError::Conflict(format!(
            "run is in terminal state '{}' and cannot be canceled",
            run.status
        )));
    }
    state
        .db
        .update_pipeline_run_status(run_id, "canceled")
        .await?;
    Ok(Json(crate::error::MessageResponse {
        message: "canceled".into(),
    }))
}

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    pub since: Option<i64>,
    pub max: Option<i64>,
}

async fn get_logs(
    State(state): State<AppState>,
    Path((_owner, _repo, _run_id, job_id)): Path<(String, String, i64, i64)>,
    Query(q): Query<LogsQuery>,
) -> ApiResult<Json<LogsResponse>> {
    let since = q.since.unwrap_or(-1);
    let max = q.max.unwrap_or(1000).clamp(1, 5000);
    let rows = state.db.list_logs_since(job_id, since, max).await?;
    Ok(Json(LogsResponse {
        lines: rows.into_iter().map(LogLineResponse::from).collect(),
    }))
}

/// SSE stream of log lines for one job.
///
/// Emits an `event: log\ndata: <line-json>\n\n` event for each new log
/// line, plus an `event: status\ndata: <status-json>\n\n` event when
/// the job transitions to a terminal state. The stream closes when
/// the job is in a terminal state AND there are no further log rows to
/// flush.
///
/// Phase 1 polls the DB at ~1 Hz to discover new rows. A future
/// revision could switch to LISTEN/NOTIFY for lower latency.
async fn stream_logs(
    State(state): State<AppState>,
    Path((_owner, _repo, _run_id, job_id)): Path<(String, String, i64, i64)>,
) -> Result<
    axum::response::sse::Sse<
        impl futures_util::Stream<Item = Result<axum::response::sse::Event, Infallible>>,
    >,
    ApiError,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use std::convert::Infallible;
    use std::time::Duration;

    let db = state.db.clone();

    // Resolve the job once so we can 404 cleanly.
    let _ = db
        .get_job(job_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("job not found".into()))?;

    // The stream polls the DB every second, emits any new log rows as
    // `event: log` events, and one `event: status` event once the job
    // reaches a terminal state. When the job is terminal AND we've
    // drained all rows, the inner stream ends and the SSE response
    // closes.
    let stream = async_stream::stream! {
        let mut next_seq: i64 = -1;
        loop {
            // Push any new log lines.
            if let Ok(rows) = db.list_logs_since(job_id, next_seq, 1000).await {
                for row in rows {
                    let seq = row.seq;
                    let payload = serde_json::to_string(&LogLineResponse::from(row))
                        .unwrap_or_else(|_| "{}".into());
                    next_seq = next_seq.max(seq + 1);
                    yield Ok::<_, Infallible>(Event::default().event("log").data(payload));
                }
            }

            // Check job status; emit `status` if terminal.
            let job_terminal = if let Ok(Some(fresh_job)) = db.get_job(job_id).await {
                let terminal = matches!(
                    fresh_job.status.as_str(),
                    "success" | "failed" | "skipped" | "canceled"
                );
                if terminal {
                    let payload = serde_json::to_string(&JobResponse::from(fresh_job))
                        .unwrap_or_else(|_| "{}".into());
                    yield Ok::<_, Infallible>(Event::default().event("status").data(payload));
                }
                terminal
            } else {
                true // job vanished — stop streaming
            };

            if job_terminal {
                break;
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::new()))
}

// ----- Envelope types ---------------------------------------------------

#[derive(Debug, Serialize)]
pub struct PipelinesResponse {
    pub pipelines: Vec<PipelineResponse>,
}

#[derive(Debug, Serialize)]
pub struct RunsResponse {
    pub runs: Vec<PipelineRunResponse>,
}

#[derive(Debug, Serialize)]
pub struct RunDetailResponse {
    pub run: PipelineRunResponse,
    pub jobs: Vec<JobResponse>,
}

#[derive(Debug, Serialize)]
pub struct LogsResponse {
    pub lines: Vec<LogLineResponse>,
}

// ----- Router -----------------------------------------------------------

pub fn pipeline_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/repos/{owner}/{repo}/pipelines",
            get(list_pipelines),
        )
        .route(
            "/api/repos/{owner}/{repo}/pipelines/{path}/runs",
            get(list_runs).post(trigger_run),
        )
        .route(
            "/api/repos/{owner}/{repo}/runs/{run_id}",
            get(get_run),
        )
        .route(
            "/api/repos/{owner}/{repo}/runs/{run_id}/cancel",
            post(cancel_run),
        )
        .route(
            "/api/repos/{owner}/{repo}/runs/{run_id}/jobs/{job_id}/logs",
            get(get_logs),
        )
        .route(
            "/api/repos/{owner}/{repo}/runs/{run_id}/jobs/{job_id}/logs/stream",
            get(stream_logs),
        )
}