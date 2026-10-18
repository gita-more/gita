-- 0002_groups.sql
--
-- Add group (namespace) support to Gita.
--
-- A group is a namespace that can own repositories, similar to GitLab
-- groups. `repositories.group_id` is nullable: NULL means a personal
-- repository owned by the user (owner_name = username).

CREATE TABLE IF NOT EXISTS groups (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT '',
    owner_id BIGINT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS group_members (
    group_id BIGINT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id BIGINT NOT NULL,
    role TEXT NOT NULL DEFAULT 'member',
    PRIMARY KEY (group_id, user_id)
);

ALTER TABLE repositories ADD COLUMN IF NOT EXISTS group_id BIGINT NULL;