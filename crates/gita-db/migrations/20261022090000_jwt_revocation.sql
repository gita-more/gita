-- 0013_jwt_revocation.sql
--
-- JWT revocation list. When a user logs out (or an admin revokes
-- sessions), we store the JWT's JTI (JWT ID) with its expiration time.
-- The `verify_jwt` function checks this list and rejects revoked tokens.

CREATE TABLE IF NOT EXISTS jwt_revocation_list (
    jti         TEXT PRIMARY KEY,
    expires_at  TIMESTAMPTZ NOT NULL,
    revoked_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS jwt_revocation_expires_idx ON jwt_revocation_list (expires_at);

-- Cleanup function: delete expired entries (older than their expires_at).
-- Can be called periodically or on each verification.
