-- 0005_comments.sql
--
-- PR / Issue comments.
--
-- A comment is attached to either a pull request (`pr_id`) or an issue
-- (`issue_id`). Exactly one of the two must be set. Comments are deleted
-- when the parent PR / issue / repository is deleted.

CREATE TABLE IF NOT EXISTS comments (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    repo_id     BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    pr_id       BIGINT REFERENCES pull_requests(id) ON DELETE CASCADE,
    issue_id    BIGINT REFERENCES issues(id) ON DELETE CASCADE,
    author_id   BIGINT NOT NULL,
    author_name TEXT NOT NULL,
    body        TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    CHECK (pr_id IS NOT NULL OR issue_id IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS comments_pr_idx    ON comments (pr_id);
CREATE INDEX IF NOT EXISTS comments_issue_idx ON comments (issue_id);
CREATE INDEX IF NOT EXISTS comments_repo_idx  ON comments (repo_id);