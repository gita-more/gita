-- 0010_pages.sql
--
-- Per-repository GitHub-Pages-style publishing configuration ("Deploy
-- from a branch"). A repository has at most one Pages site, published from
-- `branch` at `path` ("/" for the repository root or "/docs", matching
-- GitHub's constraint). There is no build step: the tree at that ref is
-- served verbatim.
--
-- `repo_id` is the primary key rather than a surrogate id because Pages is
-- strictly one-per-repo; a second PUT updates the row.

CREATE TABLE IF NOT EXISTS repo_pages (
    repo_id     BIGINT PRIMARY KEY REFERENCES repositories(id) ON DELETE CASCADE,
    enabled     BOOLEAN NOT NULL DEFAULT FALSE,
    branch      TEXT NOT NULL DEFAULT 'main',
    path        TEXT NOT NULL DEFAULT '/',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);