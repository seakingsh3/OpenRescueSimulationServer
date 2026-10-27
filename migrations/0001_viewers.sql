-- 0001_viewers.sql
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS viewers (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  name       TEXT NOT NULL,
  type       TEXT NOT NULL DEFAULT 'webrtc',
  url        TEXT,
  active     INTEGER NOT NULL DEFAULT 0,
  selected   INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
  updated_at INTEGER
);
