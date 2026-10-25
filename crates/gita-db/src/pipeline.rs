//! Data access for Gita, split by domain.

use anyhow::Result;
use chrono::Utc;
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::Db;

pub struct PipelineRow {
    pub id: i64,
    pub repo_id: i64,
    pub path: String,
    pub ref_name: String,
    pub ref_sha: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Row shape for the `pipeline_runs` table.
#[derive(Debug, Clone)]
pub struct PipelineRunRow {
    pub id: i64,
    pub pipeline_id: i64,
    pub repo_id: i64,
    pub ref_name: String,
    pub head_sha: String,
    pub trigger_event_id: Option<i64>,
    pub trigger_pusher_id: Option<i64>,
    pub status: String,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub claimed_by_runner_id: Option<String>,
    pub claimed_until: Option<String>,
}

/// Row shape for the `jobs` table. `needs` and `script` are JSON-encoded
/// arrays of strings — decoded by callers that need them.
#[derive(Debug, Clone)]
pub struct JobRow {
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

/// Row shape for the `logs` table.
#[derive(Debug, Clone)]
pub struct LogRow {
    pub id: i64,
    pub job_id: i64,
    pub seq: i64,
    pub stream: String,
    pub line: String,
    pub ts: String,
}

fn row_to_pipeline(row: &PgRow) -> Result<PipelineRow> {
    Ok(PipelineRow {
        id: row.try_get("id")?,
        repo_id: row.try_get("repo_id")?,
        path: row.try_get("path")?,
        ref_name: row.try_get("ref")?,
        ref_sha: row.try_get("ref_sha")?,
        content: row.try_get("content")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_run(row: &PgRow) -> Result<PipelineRunRow> {
    Ok(PipelineRunRow {
        id: row.try_get("id")?,
        pipeline_id: row.try_get("pipeline_id")?,
        repo_id: row.try_get("repo_id")?,
        ref_name: row.try_get("ref")?,
        head_sha: row.try_get("head_sha")?,
        trigger_event_id: row.try_get("trigger_event_id")?,
        trigger_pusher_id: row.try_get("trigger_pusher_id")?,
        status: row.try_get("status")?,
        queued_at: row.try_get("queued_at")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        claimed_by_runner_id: row.try_get("claimed_by_runner_id")?,
        claimed_until: row.try_get("claimed_until")?,
    })
}

fn row_to_job(row: &PgRow) -> Result<JobRow> {
    let needs_json: String = row.try_get("needs")?;
    let script_json: String = row.try_get("script")?;
    let needs: Vec<String> = serde_json::from_str(&needs_json).unwrap_or_default();
    let script: Vec<String> = serde_json::from_str(&script_json).unwrap_or_default();
    Ok(JobRow {
        id: row.try_get("id")?,
        run_id: row.try_get("run_id")?,
        name: row.try_get("name")?,
        stage: row.try_get("stage")?,
        needs,
        script,
        status: row.try_get("status")?,
        queued_at: row.try_get("queued_at")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        exit_code: row.try_get("exit_code")?,
    })
}

fn row_to_log(row: &PgRow) -> Result<LogRow> {
    Ok(LogRow {
        id: row.try_get("id")?,
        job_id: row.try_get("job_id")?,
        seq: row.try_get("seq")?,
        stream: row.try_get("stream")?,
        line: row.try_get("line")?,
        ts: row.try_get("ts")?,
    })
}

impl Db {
    /// Insert-or-update a pipeline record by (repo_id, path). Returns the
    /// row's id. `created_at` is only set on insert; `updated_at` is
    /// refreshed on every call.
    pub async fn upsert_pipeline(
        &self,
        repo_id: i64,
        path: &str,
        ref_name: &str,
        ref_sha: &str,
        content: &str,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO pipelines (repo_id, path, ref, ref_sha, content, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $6)
             ON CONFLICT (repo_id, path) DO UPDATE
                 SET ref = EXCLUDED.ref,
                     ref_sha = EXCLUDED.ref_sha,
                     content = EXCLUDED.content,
                     updated_at = EXCLUDED.updated_at
             RETURNING id",
        )
        .bind(repo_id)
        .bind(path)
        .bind(ref_name)
        .bind(ref_sha)
        .bind(content)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("id").unwrap_or(0))
    }

    pub async fn get_pipeline_by_path(
        &self,
        repo_id: i64,
        path: &str,
    ) -> Result<Option<PipelineRow>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, path, ref, ref_sha, content, created_at, updated_at
             FROM pipelines WHERE repo_id = $1 AND path = $2",
        )
        .bind(repo_id)
        .bind(path)
        .fetch_all(&self.pool)
        .await?;
        rows.first().map(row_to_pipeline).transpose()
    }

    pub async fn get_pipeline(&self, id: i64) -> Result<Option<PipelineRow>> {
        let rows = sqlx::query(
            "SELECT id, repo_id, path, ref, ref_sha, content, created_at, updated_at
             FROM pipelines WHERE id = $1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.first().map(row_to_pipeline).transpose()
    }

    pub async fn insert_pipeline_run(
        &self,
        pipeline_id: i64,
        repo_id: i64,
        ref_name: &str,
        head_sha: &str,
        pusher_id: Option<i64>,
        trigger_event_id: Option<i64>,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let row = sqlx::query(
            "INSERT INTO pipeline_runs
               (pipeline_id, repo_id, ref, head_sha, trigger_event_id, trigger_pusher_id,
                status, queued_at)
             VALUES ($1, $2, $3, $4, $5, $6, 'queued', $7)
             RETURNING id",
        )
        .bind(pipeline_id)
        .bind(repo_id)
        .bind(ref_name)
        .bind(head_sha)
        .bind(trigger_event_id)
        .bind(pusher_id)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("id").unwrap_or(0))
    }

    /// Attach the event that triggered a pipeline run.
    ///
    /// The run and its triggering event are created in that order: the
    /// event's payload carries the run id, so the run row has to exist
    /// first, and the run's `trigger_event_id` can only be filled in once
    /// the event row does. Returns the number of rows updated.
    pub async fn set_pipeline_run_trigger_event(&self, run_id: i64, event_id: i64) -> Result<u64> {
        let res = sqlx::query("UPDATE pipeline_runs SET trigger_event_id = $2 WHERE id = $1")
            .bind(run_id)
            .bind(event_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    pub async fn list_pipeline_runs(
        &self,
        pipeline_id: i64,
        limit: i64,
    ) -> Result<Vec<PipelineRunRow>> {
        let rows = sqlx::query(
            "SELECT id, pipeline_id, repo_id, ref, head_sha, trigger_event_id,
                    trigger_pusher_id, status, queued_at, started_at, finished_at,
                    claimed_by_runner_id, claimed_until
             FROM pipeline_runs
             WHERE pipeline_id = $1
             ORDER BY id DESC
             LIMIT $2",
        )
        .bind(pipeline_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_run).collect()
    }

    pub async fn get_pipeline_run(&self, id: i64) -> Result<Option<PipelineRunRow>> {
        let rows = sqlx::query(
            "SELECT id, pipeline_id, repo_id, ref, head_sha, trigger_event_id,
                    trigger_pusher_id, status, queued_at, started_at, finished_at,
                    claimed_by_runner_id, claimed_until
             FROM pipeline_runs WHERE id = $1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.first().map(row_to_run).transpose()
    }

    /// Atomically claim the next queued run for `runner_id`. Returns the
    /// claimed run with status now 'running' and `claimed_until` set.
    /// Returns `Ok(None)` if no run is available right now.
    ///
    /// Uses a CTE + FOR UPDATE SKIP LOCKED so multiple runners in
    /// different processes don't double-claim the same row.
    pub async fn claim_pipeline_run(
        &self,
        runner_id: &str,
        lease_seconds: i64,
    ) -> Result<Option<PipelineRunRow>> {
        let now = Utc::now().to_rfc3339();
        let lease_until = (Utc::now() + chrono::Duration::seconds(lease_seconds)).to_rfc3339();
        let rows = sqlx::query(
            "WITH next AS (
                 SELECT id FROM pipeline_runs
                 WHERE status = 'queued'
                   AND (claimed_until IS NULL OR claimed_until < $1)
                 ORDER BY queued_at
                 LIMIT 1
                 FOR UPDATE SKIP LOCKED
             )
             UPDATE pipeline_runs pr
             SET status = 'running',
                 started_at = $2,
                 claimed_by_runner_id = $3,
                 claimed_until = $4
             FROM next
             WHERE pr.id = next.id
             RETURNING pr.id, pr.pipeline_id, pr.repo_id, pr.ref, pr.head_sha,
                       pr.trigger_event_id, pr.trigger_pusher_id, pr.status,
                       pr.queued_at, pr.started_at, pr.finished_at,
                       pr.claimed_by_runner_id, pr.claimed_until",
        )
        .bind(&now)
        .bind(&now)
        .bind(runner_id)
        .bind(&lease_until)
        .fetch_all(&self.pool)
        .await?;
        rows.first().map(row_to_run).transpose()
    }

    /// Update a run's status. If `finished` is true, also stamps
    /// `finished_at` and clears `claimed_until` so the partial-restart
    /// path doesn't try to reclaim.
    pub async fn update_pipeline_run_status(&self, id: i64, status: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let terminal = matches!(status, "success" | "failed" | "canceled");
        if terminal {
            sqlx::query(
                "UPDATE pipeline_runs
                 SET status = $2,
                     finished_at = COALESCE(finished_at, $3),
                     claimed_until = NULL
                 WHERE id = $1",
            )
            .bind(id)
            .bind(status)
            .bind(&now)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query("UPDATE pipeline_runs SET status = $2 WHERE id = $1")
                .bind(id)
                .bind(status)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    pub async fn insert_job(
        &self,
        run_id: i64,
        name: &str,
        stage: &str,
        needs: &[String],
        script: &[String],
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let needs_json = serde_json::to_string(needs).unwrap_or_else(|_| "[]".into());
        let script_json = serde_json::to_string(script).unwrap_or_else(|_| "[]".into());
        let row = sqlx::query(
            "INSERT INTO jobs (run_id, name, stage, needs, script, status, queued_at)
             VALUES ($1, $2, $3, $4, $5, 'queued', $6)
             RETURNING id",
        )
        .bind(run_id)
        .bind(name)
        .bind(stage)
        .bind(needs_json)
        .bind(script_json)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("id").unwrap_or(0))
    }

    pub async fn update_job_status(
        &self,
        id: i64,
        status: &str,
        exit_code: Option<i32>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let terminal = matches!(status, "success" | "failed" | "skipped" | "canceled");
        if terminal {
            sqlx::query(
                "UPDATE jobs
                 SET status = $2,
                     exit_code = $3,
                     finished_at = COALESCE(finished_at, $4)
                 WHERE id = $1",
            )
            .bind(id)
            .bind(status)
            .bind(exit_code)
            .bind(&now)
            .execute(&self.pool)
            .await?;
        } else if status == "running" {
            sqlx::query(
                "UPDATE jobs
                 SET status = $2,
                     started_at = COALESCE(started_at, $3)
                 WHERE id = $1",
            )
            .bind(id)
            .bind(status)
            .bind(&now)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query("UPDATE jobs SET status = $2 WHERE id = $1")
                .bind(id)
                .bind(status)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    pub async fn list_jobs(&self, run_id: i64) -> Result<Vec<JobRow>> {
        let rows = sqlx::query(
            "SELECT id, run_id, name, stage, needs, script, status, queued_at,
                    started_at, finished_at, exit_code
             FROM jobs WHERE run_id = $1 ORDER BY id",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_job).collect()
    }

    pub async fn get_job(&self, id: i64) -> Result<Option<JobRow>> {
        let rows = sqlx::query(
            "SELECT id, run_id, name, stage, needs, script, status, queued_at,
                    started_at, finished_at, exit_code
             FROM jobs WHERE id = $1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.first().map(row_to_job).transpose()
    }

    pub async fn insert_log(
        &self,
        job_id: i64,
        seq: i64,
        stream: &str,
        line: &str,
        ts: &str,
    ) -> Result<i64> {
        let row = sqlx::query(
            "INSERT INTO logs (job_id, seq, stream, line, ts)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (job_id, seq) DO NOTHING
             RETURNING id",
        )
        .bind(job_id)
        .bind(seq)
        .bind(stream)
        .bind(line)
        .bind(ts)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("id").unwrap_or(0))
    }

    /// Fetch log lines for a job, starting at `since_seq` (inclusive).
    /// Pass `since_seq = -1` for the first page.
    pub async fn list_logs_since(
        &self,
        job_id: i64,
        since_seq: i64,
        max: i64,
    ) -> Result<Vec<LogRow>> {
        let rows = sqlx::query(
            "SELECT id, job_id, seq, stream, line, ts
             FROM logs
             WHERE job_id = $1 AND seq >= $2
             ORDER BY seq
             LIMIT $3",
        )
        .bind(job_id)
        .bind(since_seq)
        .bind(max)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_log).collect()
    }
}
