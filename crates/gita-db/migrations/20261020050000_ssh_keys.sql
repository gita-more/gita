-- 0009_ssh_keys.sql
--
-- SSH public keys registered by a user. The server's embedded SSH daemon
-- (gita-server) uses these to authenticate incoming `git@host:` clients.
-- A single user may have many keys (laptops, workstations, CI runners).

CREATE TABLE IF NOT EXISTS ssh_keys (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title       TEXT NOT NULL DEFAULT '',
    public_key  TEXT NOT NULL,
    created_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS ssh_keys_user_idx ON ssh_keys (user_id);
