-- 0008_password_reset_tokens.sql
--
-- Short-lived single-use tokens for the "forgot password" flow. Each row
-- is invalidated after one successful reset (`used = TRUE`) and rows
-- older than 24 hours are garbage-collected at use time.

CREATE TABLE IF NOT EXISTS password_reset_tokens (
    token       TEXT PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at  TEXT NOT NULL,
    used        BOOLEAN NOT NULL DEFAULT FALSE,
    created_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS password_reset_user_idx ON password_reset_tokens (user_id);
