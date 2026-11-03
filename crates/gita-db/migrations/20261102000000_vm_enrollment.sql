-- Node enrollment tokens and per-node credentials.
--
-- The old model had a single cluster-wide API key shared by every node
-- agent. Enrollment replaces it with the mainstream pattern used by
-- Kubernetes, GitLab Runner and Tailscale:
--
--   1. an administrator mints a one-time, short-lived join token;
--   2. the agent exchanges it for its own node id + secret;
--   3. later heartbeats/polls authenticate with that per-node secret.
--
-- Only hashes are stored. The plaintext join token is shown once by the
-- API; the plaintext node secret is shown once to the agent and written
-- to its work dir with mode 0600.

CREATE TABLE vm_enrollment_tokens (
    id                  BIGSERIAL PRIMARY KEY,
    name                TEXT NOT NULL DEFAULT '',
    -- Optional pin: when set, only an agent reporting this node name may
    -- redeem the token.
    node_name           TEXT,
    -- SHA-256 hex of the join token.
    token_hash          TEXT NOT NULL UNIQUE,
    created_at_ms       BIGINT NOT NULL,
    expires_at_ms       BIGINT NOT NULL,
    used_at_ms          BIGINT,
    used_by_node_id     BIGINT REFERENCES vm_nodes(id) ON DELETE SET NULL
);

CREATE INDEX idx_vm_enrollment_tokens_expires ON vm_enrollment_tokens(expires_at_ms);

CREATE TABLE vm_node_credentials (
    node_id         BIGINT PRIMARY KEY REFERENCES vm_nodes(id) ON DELETE CASCADE,
    -- SHA-256 hex of the per-node secret.
    secret_hash     TEXT NOT NULL,
    created_at_ms   BIGINT NOT NULL,
    updated_at_ms   BIGINT NOT NULL
);
