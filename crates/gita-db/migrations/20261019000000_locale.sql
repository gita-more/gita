-- 0003_locale.sql
-- Add per-user locale preference (frontend i18n). Default 'zh' matches
-- the current UI language and index.html lang attribute.

ALTER TABLE users ADD COLUMN IF NOT EXISTS locale TEXT NOT NULL DEFAULT 'zh';