//! Run-claim logic using Postgres FOR UPDATE SKIP LOCKED.

use gita_db::Db;
use sqlx::Row;
use std::time::Duration;

/// Claim the next queued run, stamping `status='running'`,
/// `claimed_by_runner_id`, and `claimed_until`. Returns the run row
/// directly from the `pipeline_runs` table.
pub async fn claim_one_run(
    db: &Db,
    runner_name: &str,
    lease: Duration,
) -> anyhow::Result<Option<gita_db::PipelineRunRow>> {
    let now = chrono::Utc::now().to_rfc3339();
    let lease_until =
        (chrono::Utc::now() + chrono::Duration::from_std(lease).unwrap()).to_rfc3339();

    let rows = sqlx::query(
        r#"
        WITH next AS (
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
                  pr.claimed_by_runner_id, pr.claimed_until
        "#,
    )
    .bind(&now)
    .bind(&now)
    .bind(runner_name)
    .bind(&lease_until)
    .fetch_all(db.pool())
    .await?;

    // Reuse gita-db's row_to_run helper if available — fall back to inline decoding.
    Ok(match rows.first() {
        Some(row) => Some(decode_run_row(row)?),
        None => None,
    })
}

fn decode_run_row(row: &sqlx::postgres::PgRow) -> anyhow::Result<gita_db::PipelineRunRow> {
    Ok(gita_db::PipelineRunRow {
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
