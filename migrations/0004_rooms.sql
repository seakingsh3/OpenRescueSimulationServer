-- migrations/20251119001_rooms.sql
CREATE TABLE IF NOT EXISTS rooms (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT    NOT NULL,
    layout       TEXT    NOT NULL,
    audio_device TEXT    NULL,
    sources      TEXT    NULL,  -- JSON-Array: ["rtsp://...", null, "testsrc", ...]
    viewer_id    INTEGER NULL,
    created_at   INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    updated_at   INTEGER NOT NULL DEFAULT (strftime('%s','now')),

    FOREIGN KEY(viewer_id) REFERENCES viewers(id) ON DELETE SET NULL
);
