-- 0004_pipelines.sql — In-repo CI tables for `.gita-ci.yml`.
-- See goal.md §In-repo CI.

CREATE TABLE IF NOT EXISTS pipelines (
    id          BIGSERIAL PRIMARY KEY,
    repo_id     BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    ref         TEXT NOT NULL,
    ref_sha     TEXT NOT NULL,
    content     TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    UNIQUE (repo_id, path)
);

CREATE INDEX IF NOT EXISTS pipelines_repo_idx ON pipelines (repo_id);

CREATE TABLE IF NOT EXISTS pipeline_runs (
    id                   BIGSERIAL PRIMARY KEY,
    pipeline_id          BIGINT NOT NULL REFERENCES pipelines(id) ON DELETE CASCADE,
    repo_id              BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    ref                  TEXT NOT NULL,
    head_sha             TEXT NOT NULL,
    trigger_event_id     BIGINT REFERENCES events(id) ON DELETE SET NULL,
    trigger_pusher_id    BIGINT,
    status               TEXT NOT NULL
        CHECK (status IN ('queued','running','success','failed','canceled')),
    queued_at            TEXT NOT NULL,
    started_at           TEXT,
    finished_at          TEXT,
    claimed_by_runner_id TEXT,
    claimed_until        TEXT
);

-- Partial index for the SKIP LOCKED claim scan:
CREATE INDEX IF NOT EXISTS pipeline_runs_queued_idx
    ON pipeline_runs (queued_at) WHERE status = 'queued';

CREATE INDEX IF NOT EXISTS pipeline_runs_repo_idx     ON pipeline_runs (repo_id, id DESC);
CREATE INDEX IF NOT EXISTS pipeline_runs_pipeline_idx ON pipeline_runs (pipeline_id, id DESC);

CREATE TABLE IF NOT EXISTS jobs (
    id           BIGSERIAL PRIMARY KEY,
    run_id       BIGINT NOT NULL REFERENCES pipeline_runs(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    stage        TEXT NOT NULL,
    needs        TEXT NOT NULL,
    script       TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued','running','success','failed','skipped','canceled')),
    queued_at    TEXT NOT NULL,
    started_at   TEXT,
    finished_at  TEXT,
    exit_code    INTEGER,
    UNIQUE (run_id, name)
);

CREATE INDEX IF NOT EXISTS jobs_run_idx ON jobs (run_id, id);

CREATE TABLE IF NOT EXISTS logs (
    id      BIGSERIAL PRIMARY KEY,
    job_id  BIGINT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    seq     BIGINT NOT NULL,
    stream  TEXT NOT NULL CHECK (stream IN ('stdout','stderr','system')),
    line    TEXT NOT NULL,
    ts      TEXT NOT NULL,
    UNIQUE (job_id, seq)
);

CREATE INDEX IF NOT EXISTS logs_job_seq_idx ON logs (job_id, seq);