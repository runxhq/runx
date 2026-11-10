-- The kernel store as runx 0.11.1 initializes it, from
-- crates/runx-runtime/src/kernel/store/schema.rs at cli-v0.11.1: in WAL mode,
-- its schema, then its mark and its format, 2.
PRAGMA journal_mode = WAL;
CREATE TABLE kernel_streams (
  kind TEXT NOT NULL,
  key TEXT NOT NULL,
  version INTEGER NOT NULL CHECK (version > 0),
  event_type TEXT NOT NULL,
  chain TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (kind, key)
) WITHOUT ROWID;

CREATE TABLE kernel_events (
  kind TEXT NOT NULL,
  key TEXT NOT NULL,
  version INTEGER NOT NULL CHECK (version > 0),
  event_type TEXT NOT NULL,
  data BLOB NOT NULL,
  chain TEXT NOT NULL,
  committed_at TEXT NOT NULL,
  PRIMARY KEY (kind, key, version)
) WITHOUT ROWID;

CREATE INDEX kernel_streams_recent ON kernel_streams (kind, updated_at, key);

CREATE TABLE kernel_blobs (
  digest TEXT NOT NULL PRIMARY KEY,
  data BLOB NOT NULL
);
PRAGMA application_id = 1381322315;
PRAGMA user_version = 2;
