-- 0007_webhooks.sql
--
-- Per-repository webhook subscriptions. When an event the repository
-- considers interesting is published via the event bus, a background
-- task fans it out as an HTTP POST to every active hook URL.

CREATE TABLE IF NOT EXISTS webhooks (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    repo_id     BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    url         TEXT NOT NULL,
    secret      TEXT NOT NULL DEFAULT '',
    events      TEXT NOT NULL DEFAULT 'push',
    active      BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS webhooks_repo_idx ON webhooks (repo_id) WHERE active;
