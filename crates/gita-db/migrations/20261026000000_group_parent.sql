-- 20261026000000_group_parent.sql
--
-- Add parent_id and full_path to groups table to support nested subgroups (GitLab-style).

ALTER TABLE groups ADD COLUMN IF NOT EXISTS parent_id BIGINT NULL REFERENCES groups(id) ON DELETE CASCADE;
ALTER TABLE groups ADD COLUMN IF NOT EXISTS full_path TEXT NOT NULL DEFAULT '';

-- Index for finding subgroups of a group
CREATE INDEX IF NOT EXISTS idx_groups_parent_id ON groups(parent_id);

-- Update the unique constraint on name to be scoped to parent_id
-- (groups can have the same name under different parents)
-- We need to drop and recreate the unique constraint
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'groups_name_key' AND conrelid = 'groups'::regclass
    ) THEN
        ALTER TABLE groups DROP CONSTRAINT groups_name_key;
    END IF;
END $$;

CREATE UNIQUE INDEX IF NOT EXISTS idx_groups_name_parent ON groups(name, parent_id);

-- Backfill full_path for existing groups (root groups have full_path = name)
UPDATE groups SET full_path = name WHERE full_path = '';