-- 0012_access_tokens_hash.sql
--
-- Personal access tokens were stored in plaintext, and `GET
-- /api/auth/tokens` returned the secret again on every call. A leaked
-- database backup — or a leaked API response, a proxy log, a screen
-- share — therefore handed out working credentials.
--
-- New tokens are stored as a SHA-256 hash in `token_hash`; the
-- plaintext is shown to the user exactly once, when the token is
-- created. The hash is computed in the application (see
-- `gita_db::hash_token`) rather than in SQL, so this migration does
-- not depend on a database extension.
--
-- Rows written before this migration still hold their plaintext in
-- `token`. They keep working — the application's lookup falls back to
-- the plaintext column — and are converted to a hash the first time
-- they are used, so the plaintext is not left in the database
-- indefinitely.
--
-- The unique index is on the hash only. The legacy `token` column's
-- UNIQUE constraint is dropped so that new tokens can all store an
-- empty string there.

ALTER TABLE access_tokens ADD COLUMN IF NOT EXISTS token_hash TEXT;

-- Drop the legacy UNIQUE constraint on `token` so that new tokens
-- can all write an empty string without colliding.
-- (The constraint is named `access_tokens_token_key` by Postgres.)
ALTER TABLE access_tokens DROP CONSTRAINT IF EXISTS access_tokens_token_key;

-- The `token` column itself is kept (NOT NULL) for legacy row
-- compatibility, but it no longer has a uniqueness requirement.

CREATE UNIQUE INDEX IF NOT EXISTS access_tokens_hash_uniq
    ON access_tokens (token_hash)
    WHERE token_hash IS NOT NULL;
