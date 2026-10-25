-- 0013_user_admin.sql
--
-- Add a system-admin flag to users, so that one (or more) accounts can
-- manage other users (list/create/delete) from an admin settings page.
--
-- The very first account, created by POST /api/auth/init, is promoted to
-- administrator after creation (see `gita-api/src/routes/auth.rs`). All
-- other accounts — self-registered or created by an admin — start as
-- non-admin unless the admin explicitly grants the role.

ALTER TABLE users ADD COLUMN IF NOT EXISTS is_admin BOOLEAN NOT NULL DEFAULT FALSE;