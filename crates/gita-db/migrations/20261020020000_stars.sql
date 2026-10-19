-- 0006_stars.sql
--
-- A star is a (user, repo) pair indicating that the user starred the
-- repository. One row per pair; the unique index prevents duplicates.

CREATE TABLE IF NOT EXISTS stars (
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    repo_id     BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (user_id, repo_id)
);

CREATE INDEX IF NOT EXISTS stars_repo_idx ON stars (repo_id);
