-- 0011_ssh_keys_unique.sql
--
-- A user must not be able to register the same public key twice. Without
-- a unique constraint the table accumulated duplicate rows (the add-key
-- endpoint did an unconditional INSERT), which showed up as repeated
-- entries in the key list and made the fingerprint lookup — a full table
-- scan that recomputes a SHA-256 per row — slower for every SSH
-- authentication attempt.
--
-- The constraint is added after de-duplicating, so an existing database
-- that already holds duplicates is repaired rather than failing the
-- migration.

DELETE FROM ssh_keys a
      USING ssh_keys b
      WHERE a.id > b.id
        AND a.user_id = b.user_id
        AND a.public_key = b.public_key;

CREATE UNIQUE INDEX IF NOT EXISTS ssh_keys_user_key_uniq
    ON ssh_keys (user_id, public_key);
