-- D1 schema for loki sync.
--
-- The server never sees plaintext: `envelope` is the AES-256-GCM blob the client
-- already stores locally, and every other column is a version/ordering fact.
-- The Cloudflare account holder can read this table, so nothing here may be
-- secret in a way that matters if they do.

CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS items (
  id         TEXT PRIMARY KEY,
  -- What the item is (`host`, `key`, `provider`, ...). Needed on pull so a
  -- joining device can tell them apart without decrypting anything.
  kind       TEXT NOT NULL DEFAULT '',
  -- NULL means a tombstone: the item was deleted on some device.
  envelope   TEXT,
  revision   INTEGER NOT NULL DEFAULT 1,
  -- Monotonic per-vault counter the client uses as a pull cursor.
  seq        INTEGER NOT NULL,
  deleted    INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_items_seq ON items (seq);

CREATE TABLE IF NOT EXISTS sessions (
  token      TEXT PRIMARY KEY,
  device_id  TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_expiry ON sessions (expires_at);
