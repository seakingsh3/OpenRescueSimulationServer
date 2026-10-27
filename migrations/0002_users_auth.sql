-- Ensure users table exists with auth columns.
-- Ergänzung für Installationen ohne neue Spalten; eigentliche ALTERs werden
-- zur Laufzeit in init_db() nachgezogen.
CREATE TABLE IF NOT EXISTS users(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT UNIQUE,
    password_hash TEXT NOT NULL DEFAULT '',
    role TEXT NOT NULL DEFAULT 'admin',
    created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    updated_at INTEGER
);
