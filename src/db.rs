use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::Row;
use sqlx::{FromRow, SqlitePool};
use sqlx::{Pool, Sqlite};
use std::{
    env, fs,
    path::PathBuf,
};

/// App-Verzeichnisse (~/.local/share/openresquesim/…)
#[allow(dead_code)]
pub struct AppDirs {
    pub base_dir: PathBuf,
    pub data_dir: PathBuf,
    pub rec_dir: PathBuf,
    pub db_file: PathBuf,
}

#[derive(Debug, Serialize, Clone, FromRow)]
pub struct Camera {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub mjpeg_url: Option<String>,
    pub fov_deg: Option<f64>,
    pub enabled: i64,
    pub created_at: i64,
    pub brand: String,
    pub ptz_capable: i64,
    #[serde(skip_serializing)]
    pub vapix_username: Option<String>,
    #[serde(skip_serializing)]
    pub vapix_password: Option<String>,
    pub vapix_port: Option<i64>,
}

pub fn app_dirs() -> AppDirs {
    let base = app_data_dir();
    let data = base.join("data/db");
    let rec = base.join("recordings");
    let db = env::var("ORS_DB_PATH")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| base.join("app.db"));

    if let Some(parent) = db.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::create_dir_all(&data);
    let _ = fs::create_dir_all(&rec);
    AppDirs {
        base_dir: base,
        data_dir: data,
        rec_dir: rec,
        db_file: db,
    }
}

pub async fn init_db(db_url: &str) -> Result<SqlitePool> {
    let pool = SqlitePool::connect(db_url).await?;

    // --- PRAGMA ---
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;
    sqlx::query("PRAGMA journal_mode = WAL")
        .execute(&pool)
        .await?;
    sqlx::query("PRAGMA busy_timeout = 3000")
        .execute(&pool)
        .await?;

    // --- settings (key/value) ---
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS settings(
            key TEXT PRIMARY KEY,
            value TEXT,
            updated_at INTEGER DEFAULT (strftime('%s','now'))
        )"#,
    )
    .execute(&pool)
    .await?;

    // --- sessions ---
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS sessions(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            record_is_active INTEGER DEFAULT 0,
            start_time INTEGER DEFAULT 0,
            stop_time  INTEGER DEFAULT 0,
            length     INTEGER DEFAULT 0,
            filename   TEXT
        )"#,
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_active ON sessions(record_is_active)")
        .execute(&pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_start ON sessions(start_time)")
        .execute(&pool)
        .await?;

    // --- users (für Auth) ---
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS users(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT UNIQUE,
            password_hash TEXT NOT NULL DEFAULT '',
            role TEXT NOT NULL DEFAULT 'admin',
            created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            updated_at INTEGER
        )"#,
    )
    .execute(&pool)
    .await?;
    // Legacy-Datenbank ohne Spalten nachziehen (Fehler ignorieren, falls Spalte existiert)
    let _ = sqlx::query("ALTER TABLE users ADD COLUMN password_hash TEXT NOT NULL DEFAULT ''")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'admin'")
        .execute(&pool)
        .await;
    let _ = sqlx::query(
        "ALTER TABLE users ADD COLUMN created_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))",
    )
    .execute(&pool)
    .await;
    let _ = sqlx::query("ALTER TABLE users ADD COLUMN updated_at INTEGER")
        .execute(&pool)
        .await;

    // --- invite_tokens (self-register) ---
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS invite_tokens(
            token TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            created_by INTEGER,
            used_by INTEGER,
            used_at INTEGER,
            FOREIGN KEY(created_by) REFERENCES users(id) ON DELETE SET NULL,
            FOREIGN KEY(used_by) REFERENCES users(id) ON DELETE SET NULL
        )"#,
    )
    .execute(&pool)
    .await?;

    // --- participant_sheets (Teilnehmer-Notizblätter) ---
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS participant_sheets(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            token TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            data TEXT NOT NULL DEFAULT '{}',
            session_id INTEGER,
            created_by INTEGER,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            updated_at INTEGER
        )"#,
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_participant_sheets_session ON participant_sheets(session_id)")
        .execute(&pool)
        .await?;

    // --- speakers (Voice of Room: selbst registrierte Raumlautsprecher) ---
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS speakers(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            device_id TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            group_name TEXT NOT NULL DEFAULT 'default',
            volume INTEGER NOT NULL DEFAULT 80,
            muted INTEGER NOT NULL DEFAULT 0,
            last_ip TEXT,
            last_seen INTEGER,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
        )"#,
    )
    .execute(&pool)
    .await?;

    // --- marker_types ---
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS marker_types(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            color TEXT NOT NULL DEFAULT '#ff0000',
            user_id INTEGER,
            sort_index INTEGER,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            updated_at INTEGER
        )"#,
    )
    .execute(&pool)
    .await?;
    let _ = sqlx::query("ALTER TABLE marker_types ADD COLUMN user_id INTEGER")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE marker_types ADD COLUMN sort_index INTEGER")
        .execute(&pool)
        .await;
    // Tastenkürzel je Marker-Typ (z.B. "F13", "Numpad1", "Ctrl+Shift+Digit1")
    let _ = sqlx::query("ALTER TABLE marker_types ADD COLUMN hotkey TEXT")
        .execute(&pool)
        .await;
    let _ = sqlx::query("CREATE INDEX IF NOT EXISTS idx_marker_types_user ON marker_types(user_id)")
        .execute(&pool)
        .await;

    // --- markers ---
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS markers(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id INTEGER NOT NULL,
            marker_type_id INTEGER NOT NULL,
            record_time INTEGER NOT NULL,
            preselected INTEGER NOT NULL DEFAULT 0,
            user_id INTEGER,
            text TEXT,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            updated_at INTEGER,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE,
            FOREIGN KEY(marker_type_id) REFERENCES marker_types(id) ON DELETE CASCADE,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE SET NULL
        )"#,
    )
    .execute(&pool)
    .await?;
    let _ = sqlx::query("ALTER TABLE markers ADD COLUMN preselected INTEGER NOT NULL DEFAULT 0")
        .execute(&pool)
        .await;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_markers_session ON markers(session_id)")
        .execute(&pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_markers_type ON markers(marker_type_id)")
        .execute(&pool)
        .await?;

    // --- achievements / user stats ---
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS achievements(
            key TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT NOT NULL,
            points INTEGER NOT NULL DEFAULT 0,
            hidden INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
        )"#,
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS user_achievements(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL,
            achievement_key TEXT NOT NULL,
            unlocked_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            context TEXT,
            UNIQUE(user_id, achievement_key),
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
            FOREIGN KEY(achievement_key) REFERENCES achievements(key) ON DELETE CASCADE
        )"#,
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_user_achievements_user ON user_achievements(user_id)")
        .execute(&pool)
        .await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS user_stats(
            user_id INTEGER PRIMARY KEY,
            markers_total INTEGER NOT NULL DEFAULT 0,
            markers_text_total INTEGER NOT NULL DEFAULT 0,
            sim_starts_total INTEGER NOT NULL DEFAULT 0,
            debriefs_total INTEGER NOT NULL DEFAULT 0,
            last_marker_at INTEGER,
            last_sim_start_at INTEGER,
            last_sim_stop_at INTEGER,
            last_sim_length_ms INTEGER,
            last_debrief_at INTEGER,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        )"#,
    )
    .execute(&pool)
    .await?;

    // sessions hast du schon; wichtig: start_time = INTEGER (Unix s), record_is_active = 0/1

    // --- cameras (Quellen) ---
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS cameras(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            url  TEXT NOT NULL,
            mjpeg_url TEXT,
            fov_deg REAL,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at INTEGER DEFAULT (strftime('%s','now')),
            brand TEXT NOT NULL DEFAULT 'generic',
            ptz_capable INTEGER NOT NULL DEFAULT 0,
            vapix_username TEXT,
            vapix_password TEXT,
            vapix_port INTEGER
        )"#,
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_cameras_name ON cameras(name)")
        .execute(&pool)
        .await?;
    let _ = sqlx::query("ALTER TABLE cameras ADD COLUMN mjpeg_url TEXT")
        .execute(&pool)
        .await;
    // PTZ (Axis/VAPIX) — defensiv für Bestandsinstallationen nachgezogen.
    let _ = sqlx::query("ALTER TABLE cameras ADD COLUMN brand TEXT NOT NULL DEFAULT 'generic'")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE cameras ADD COLUMN ptz_capable INTEGER NOT NULL DEFAULT 0")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE cameras ADD COLUMN vapix_username TEXT")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE cameras ADD COLUMN vapix_password TEXT")
        .execute(&pool)
        .await;
    let _ = sqlx::query("ALTER TABLE cameras ADD COLUMN vapix_port INTEGER")
        .execute(&pool)
        .await;

    // --- transcode_jobs (Post-Processing Queue) ---
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS transcode_jobs(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id INTEGER NOT NULL,
            input_path TEXT NOT NULL,
            out_dir    TEXT NOT NULL,
            mode       TEXT NOT NULL,        -- 'mp4' | 'hls' | 'dash'
            status     TEXT NOT NULL,        -- 'pending'|'running'|'done'|'failed'
            progress   REAL DEFAULT 0,       -- 0..100
            log        TEXT,
            created_at INTEGER DEFAULT (strftime('%s','now')),
            started_at INTEGER,
            finished_at INTEGER,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        )"#,
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_jobs_status ON transcode_jobs(status)")
        .execute(&pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_jobs_session ON transcode_jobs(session_id)")
        .execute(&pool)
        .await?;

    // --- vod_items (veröffentlichte VODs pro Session/Variante) ---
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS vod_items(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id INTEGER NOT NULL,
            kind TEXT NOT NULL,             -- 'mp4'|'hls'|'dash'
            label TEXT,                     -- z.B. '720p', 'master'
            file_path TEXT,                 -- lokaler Pfad
            url TEXT,                       -- öffentlicher Pfad (/vod/..)
            size_bytes INTEGER,
            duration_sec REAL,
            created_at INTEGER DEFAULT (strftime('%s','now')),
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        )"#,
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_vod_session ON vod_items(session_id)")
        .execute(&pool)
        .await?;

    // --- rooms (vordefinierte Räume/Layouts) ---
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS rooms(
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            name         TEXT    NOT NULL,
            layout       TEXT    NOT NULL,
            audio_device TEXT    NULL,
            sources      TEXT    NULL,  -- JSON-Array: ["rtsp://...", null, "testsrc", ...]
            viewer_id    INTEGER NULL,
            speakers     TEXT    NULL,  -- JSON-Array der Lautsprecher-IDs (speakers.id)
            created_at   INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            updated_at   INTEGER NOT NULL DEFAULT (strftime('%s','now'))
        )"#,
    )
    .execute(&pool)
    .await?;
    // Bestehende Datenbanken: Lautsprecher je Raum nachrüsten
    let _ = sqlx::query("ALTER TABLE rooms ADD COLUMN speakers TEXT NULL")
        .execute(&pool)
        .await;

    crate::achievement::seed_defs(&pool).await?;

    Ok(pool)
}

pub async fn settings_ensure(
    pool: &sqlx::SqlitePool,
    key: &str,
    default: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO settings (key, value, updated_at)
        SELECT ?, ?, strftime('%s','now')
        WHERE NOT EXISTS (SELECT 1 FROM settings WHERE key = ?)
        "#,
    )
    .bind(key)
    .bind(default)
    .bind(key)
    .execute(pool)
    .await?;
    Ok(())
}

// ---- Settings ----

pub async fn settings_set(pool: &SqlitePool, key: &str, value: &str) -> Result<()> {
    sqlx::query("INSERT INTO settings(key,value,updated_at) VALUES(?,?,strftime('%s','now')) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at")
        .bind(key).bind(value).execute(pool).await?;
    Ok(())
}

pub async fn settings_get(pool: &SqlitePool, key: &str) -> Result<Option<String>> {
    let row = sqlx::query_as::<_, (String,)>("SELECT value FROM settings WHERE key=?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|t| t.0))
}

#[derive(FromRow, Debug, Clone, serde::Serialize)]
pub struct SessionRow {
    pub id: i64,
    pub name: Option<String>,
    pub record_is_active: Option<i64>, // 0/1 in SQLite
    pub start_time: Option<i64>,       // INTEGER (Unix-Timestamp?)
    pub stop_time: Option<i64>,        // INTEGER
    pub length: Option<i64>,           // INTEGER (Sekunden)
    pub filename: Option<String>,
}

pub async fn sessions_list(db: &Pool<Sqlite>) -> Result<Vec<SessionRow>> {
    // „filename“ ist ein Tippfehler in der DB – wir aliasen auf „filename“
    let rows = sqlx::query_as::<_, SessionRow>(
        r#"
        SELECT
            id,
            name,
            record_is_active,
            start_time,
            stop_time,
            length,
            filename AS filename
        FROM sessions
        ORDER BY id DESC
        "#,
    )
    .fetch_all(db)
    .await
    .context("fetch sessions")?;
    Ok(rows)
}

pub async fn session_by_id(db: &Pool<Sqlite>, id: i64) -> Result<Option<SessionRow>> {
    let row = sqlx::query_as::<_, SessionRow>(
        r#"
        SELECT
            id,
            name,
            record_is_active,
            start_time,
            stop_time,
            length,
            filename AS filename
        FROM sessions
        WHERE id = ?1
        "#,
    )
    .bind(id)
    .fetch_optional(db)
    .await
    .context("fetch session by id")?;
    Ok(row)
}

/// Löscht Marker + Session in EINER Transaktion.
/// (Wenn du FK + ON DELETE CASCADE hast, reicht das DELETE auf sessions.)
pub async fn delete_session_in_txn(db: &Pool<Sqlite>, id: i64) -> Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query(r#"DELETE FROM markers WHERE session_id = ?1"#)
        .bind(id)
        .execute(&mut *tx)
        .await
        .context("delete markers")?;
    sqlx::query(r#"DELETE FROM vod_items WHERE session_id = ?1"#)
        .bind(id)
        .execute(&mut *tx)
        .await
        .context("delete vod_items")?;
    sqlx::query(r#"DELETE FROM transcode_jobs WHERE session_id = ?1"#)
        .bind(id)
        .execute(&mut *tx)
        .await
        .context("delete transcode_jobs")?;

    sqlx::query(r#"DELETE FROM sessions WHERE id = ?1"#)
        .bind(id)
        .execute(&mut *tx)
        .await
        .context("delete session")?;

    tx.commit().await?;
    Ok(())
}

pub async fn delete_all_sessions_in_txn(db: &Pool<Sqlite>) -> Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query(r#"DELETE FROM markers"#)
        .execute(&mut *tx)
        .await
        .context("delete all markers")?;
    sqlx::query(r#"DELETE FROM vod_items"#)
        .execute(&mut *tx)
        .await
        .context("delete all vod_items")?;
    sqlx::query(r#"DELETE FROM transcode_jobs"#)
        .execute(&mut *tx)
        .await
        .context("delete all transcode_jobs")?;
    sqlx::query(r#"DELETE FROM sessions"#)
        .execute(&mut *tx)
        .await
        .context("delete all sessions")?;
    tx.commit().await?;
    Ok(())
}

// ---- Users ----
#[derive(Debug, Serialize, Clone, FromRow)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub password_hash: Option<String>,
    pub role: String,
    pub created_at: i64,
    pub updated_at: Option<i64>,
}

pub async fn users_list(pool: &SqlitePool) -> Result<Vec<UserRow>> {
    Ok(sqlx::query_as::<_, UserRow>(
        r#"SELECT id, username, password_hash, role, created_at, updated_at
           FROM users
           ORDER BY id ASC"#,
    )
    .fetch_all(pool)
    .await?)
}

pub async fn user_by_username(pool: &SqlitePool, username: &str) -> Result<Option<UserRow>> {
    sqlx::query_as::<_, UserRow>(
        r#"SELECT id, username, password_hash, role, created_at, updated_at
           FROM users
           WHERE username = ?"#,
    )
    .bind(username)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn user_by_id(pool: &SqlitePool, id: i64) -> Result<Option<UserRow>> {
    sqlx::query_as::<_, UserRow>(
        r#"SELECT id, username, password_hash, role, created_at, updated_at
           FROM users
           WHERE id = ?"#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn user_insert(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
    role: &str,
) -> Result<UserRow> {
    let res = sqlx::query(r#"INSERT INTO users (username, password_hash, role) VALUES (?, ?, ?)"#)
        .bind(username)
        .bind(password_hash)
        .bind(role)
        .execute(pool)
        .await?;
    let id = res.last_insert_rowid();
    user_by_id(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("user insert failed"))
}

pub async fn user_delete(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query(r#"DELETE FROM users WHERE id = ?"#)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, Serialize, Clone, FromRow)]
pub struct InviteTokenRow {
    pub token: String,
    pub created_at: i64,
    pub created_by: Option<i64>,
    pub used_by: Option<i64>,
    pub used_at: Option<i64>,
}

pub async fn invite_token_create(pool: &SqlitePool, created_by: Option<i64>) -> Result<String> {
    use rand_core::RngCore;
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut token = String::new();
    let mut buf = [0u8; 8];
    let mut attempts = 0;
    loop {
        attempts += 1;
        if attempts > 10 {
            anyhow::bail!("failed to generate unique token");
        }
        rand_core::OsRng.fill_bytes(&mut buf);
        token.clear();
        for b in &buf[..5] {
            let idx = (*b as usize) % CHARS.len();
            token.push(CHARS[idx] as char);
        }
        let res = sqlx::query(
            r#"INSERT INTO invite_tokens (token, created_by) VALUES (?1, ?2)"#,
        )
        .bind(&token)
        .bind(created_by)
        .execute(pool)
        .await;
        if res.is_ok() {
            return Ok(token);
        }
    }
}

pub async fn invite_token_get(pool: &SqlitePool, token: &str) -> Result<Option<InviteTokenRow>> {
    sqlx::query_as::<_, InviteTokenRow>(
        r#"SELECT token, created_at, created_by, used_by, used_at
           FROM invite_tokens
           WHERE token = ?1"#,
    )
    .bind(token)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn invite_token_mark_used(
    pool: &SqlitePool,
    token: &str,
    user_id: i64,
) -> Result<bool> {
    let res = sqlx::query(
        r#"UPDATE invite_tokens
           SET used_by = ?2, used_at = strftime('%s','now')
           WHERE token = ?1 AND used_at IS NULL"#,
    )
    .bind(token)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn ensure_admin_user(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
) -> Result<()> {
    // Immer sicherstellen: admin vorhanden und Passwort gesetzt
    if let Some(existing) = user_by_username(pool, username).await? {
        sqlx::query("UPDATE users SET password_hash = ?, role = 'admin', updated_at = strftime('%s','now') WHERE id = ?")
            .bind(password_hash)
            .bind(existing.id)
            .execute(pool)
            .await?;
    } else {
        let _ = user_insert(pool, username, password_hash, "admin").await?;
    }
    Ok(())
}

// ---- Markers ----
#[derive(Debug, Serialize, Clone, FromRow)]
pub struct MarkerType {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub user_id: Option<i64>,
    pub sort_index: Option<i64>,
    /// Tastenkürzel (normalisiert, z.B. "F13", "Numpad1", "Ctrl+Shift+Digit1")
    pub hotkey: Option<String>,
}

pub fn marker_type_default_sort_index(name: &str) -> Option<i64> {
    match name.trim().to_ascii_lowercase().as_str() {
        "richtig" => Some(1),
        "falsch" => Some(2),
        "info" => Some(3),
        "warnung" => Some(4),
        _ => None,
    }
}

#[derive(Debug, Serialize, Clone, FromRow)]
pub struct MarkerJoined {
    pub id: i64,
    pub timestamp: i64,
    pub text: Option<String>,
    pub marker_type: String,
    pub color: String,
    pub preselected: i64,
    pub user_id: Option<i64>,
    pub username: Option<String>,
}

#[derive(FromRow, Debug, Clone, serde::Serialize)]
pub struct MarkerRow {
    pub id: i64,
    pub session_id: i64,
    pub marker_type_id: i64,
    pub record_time: i64,
    pub preselected: i64,
    pub user_id: Option<i64>,
    pub text: Option<String>,
}

pub async fn marker_types_list(pool: &SqlitePool, user_id: Option<i64>) -> Result<Vec<MarkerType>> {
    if let Some(uid) = user_id {
        Ok(sqlx::query_as::<_, MarkerType>(
            r#"SELECT id, name, color, user_id, sort_index, hotkey
               FROM marker_types
               WHERE user_id = ?1 OR user_id IS NULL
               ORDER BY CASE WHEN user_id = ?1 THEN 0 ELSE 1 END,
                        COALESCE(sort_index, CASE lower(name)
                            WHEN 'richtig' THEN 1
                            WHEN 'falsch' THEN 2
                            WHEN 'info' THEN 3
                            WHEN 'warnung' THEN 4
                            ELSE 999999
                        END),
                        name ASC"#,
        )
        .bind(uid)
        .fetch_all(pool)
        .await?)
    } else {
        Ok(sqlx::query_as::<_, MarkerType>(
            r#"SELECT id, name, color, user_id, sort_index, hotkey
               FROM marker_types
               WHERE user_id IS NULL
               ORDER BY COALESCE(sort_index, CASE lower(name)
                   WHEN 'richtig' THEN 1
                   WHEN 'falsch' THEN 2
                   WHEN 'info' THEN 3
                   WHEN 'warnung' THEN 4
                   ELSE 999999
               END), name ASC"#,
        )
        .fetch_all(pool)
        .await?)
    }
}

pub async fn marker_type_get(pool: &SqlitePool, id: i64) -> Result<Option<MarkerType>> {
    Ok(sqlx::query_as::<_, MarkerType>(
        "SELECT id, name, color, user_id, sort_index, hotkey FROM marker_types WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

pub async fn marker_types_next_sort_index(
    pool: &SqlitePool,
    user_id: Option<i64>,
) -> Result<i64> {
    if let Some(uid) = user_id {
        let row: (Option<i64>,) = sqlx::query_as(
            "SELECT MAX(sort_index) FROM marker_types WHERE user_id = ?1",
        )
        .bind(uid)
        .fetch_one(pool)
        .await?;
        Ok(row.0.unwrap_or(0) + 1)
    } else {
        let row: (Option<i64>,) = sqlx::query_as(
            "SELECT MAX(sort_index) FROM marker_types WHERE user_id IS NULL",
        )
        .fetch_one(pool)
        .await?;
        Ok(row.0.unwrap_or(0) + 1)
    }
}

pub async fn marker_types_add(
    pool: &SqlitePool,
    name: &str,
    color: &str,
    user_id: Option<i64>,
    sort_index: Option<i64>,
) -> Result<i64> {
    let res = sqlx::query(
        "INSERT INTO marker_types (name, color, user_id, sort_index) VALUES (?, ?, ?, ?)",
    )
    .bind(name)
    .bind(color)
    .bind(user_id)
    .bind(sort_index)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}
pub async fn marker_types_delete(pool: &SqlitePool, id: i64) -> Result<bool> {
    let r = sqlx::query("DELETE FROM marker_types WHERE id=?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

/// Tastenkürzel eines Marker-Typs setzen oder löschen (None/leer = kein Kürzel).
pub async fn marker_types_set_hotkey(
    pool: &SqlitePool,
    id: i64,
    hotkey: Option<&str>,
) -> Result<bool> {
    let hk = hotkey.map(str::trim).filter(|h| !h.is_empty());
    let r = sqlx::query("UPDATE marker_types SET hotkey = ? WHERE id = ?")
        .bind(hk)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn marker_types_update(
    pool: &SqlitePool,
    id: i64,
    name: &str,
    color: &str,
    sort_index: Option<i64>,
) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE marker_types SET name = ?, color = ?, sort_index = ? WHERE id = ?",
    )
    .bind(name)
    .bind(color)
    .bind(sort_index)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn marker_add(
    pool: &SqlitePool,
    session_id: i64,
    marker_type_id: i64,
    user_id: Option<i64>,
    text: Option<String>,
    record_time: Option<i64>,
) -> Result<i64> {
    let ts = record_time.unwrap_or_else(|| {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    });
    let res = sqlx::query(
        "INSERT INTO markers (session_id, marker_type_id, record_time, user_id, text, preselected) VALUES (?, ?, ?, ?, ?, 0)"
    ).bind(session_id).bind(marker_type_id).bind(ts).bind(user_id).bind(text).execute(pool).await?;
    Ok(res.last_insert_rowid())
}

pub async fn list_markers_by_session(
    db: &SqlitePool,
    session_id: i64,
) -> Result<Vec<MarkerJoined>> {
    Ok(sqlx::query_as::<_, MarkerJoined>(
        r#"
        SELECT m.id AS id,
               m.record_time AS timestamp,
               m.text AS text,
               t.name AS marker_type,
               t.color AS color,
               m.preselected AS preselected,
               m.user_id AS user_id,
               u.username AS username
        FROM markers m
        JOIN marker_types t ON t.id = m.marker_type_id
        LEFT JOIN users u ON u.id = m.user_id
        WHERE m.session_id = ?
        ORDER BY m.record_time ASC
        "#,
    )
    .bind(session_id)
    .fetch_all(db) // <── hier war dein Tippfehler
    .await?)
}
pub async fn markers_list_joined_by_session(
    pool: &SqlitePool,
    session_id: i64,
) -> Result<Vec<MarkerJoined>> {
    Ok(sqlx::query_as::<_, MarkerJoined>(
        r#"
        SELECT m.id AS id, m.record_time AS timestamp, m.text AS text,
               t.name AS marker_type, t.color AS color, m.preselected AS preselected, m.user_id AS user_id, u.username AS username
        FROM markers m
        JOIN marker_types t ON t.id = m.marker_type_id
        LEFT JOIN users u ON u.id = m.user_id
        WHERE m.session_id = ?
        ORDER BY m.record_time ASC
    "#,
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?)
}

pub async fn marker_by_id(pool: &SqlitePool, marker_id: i64) -> Result<Option<MarkerRow>> {
    sqlx::query_as::<_, MarkerRow>(
        r#"SELECT id, session_id, marker_type_id, record_time, preselected, user_id, text
           FROM markers
           WHERE id = ?"#,
    )
    .bind(marker_id)
    .fetch_optional(pool)
    .await
    .context("marker by id")
}

pub async fn marker_delete(pool: &SqlitePool, id: i64) -> Result<bool> {
    let r = sqlx::query("DELETE FROM markers WHERE id=?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}


pub async fn update_marker_text(
    pool: &SqlitePool,
    marker_id: i64,
    text: Option<String>,
) -> Result<()> {
    sqlx::query(r#"UPDATE markers SET text = ? WHERE id = ?"#)
        .bind(text)
        .bind(marker_id)
        .execute(pool)
        .await
        .context("update marker text")?;
    Ok(())
}

pub async fn update_marker_preselected(
    pool: &SqlitePool,
    marker_id: i64,
    preselected: bool,
) -> Result<()> {
    sqlx::query(r#"UPDATE markers SET preselected = ? WHERE id = ?"#)
        .bind(preselected as i64)
        .bind(marker_id)
        .execute(pool)
        .await
        .context("update marker preselected")?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct SimSessionRow {
    pub id: i64,
    pub name: Option<String>,
    pub start_time: Option<i64>,
}

pub async fn sim_active_session_id(pool: &SqlitePool) -> Result<Option<i64>> {
    let row = sqlx::query("SELECT id FROM sessions WHERE record_is_active = 1 LIMIT 1")
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get::<i64, _>("id")))
}

pub async fn sim_active_session_row(pool: &SqlitePool) -> Result<Option<SimSessionRow>> {
    let row = sqlx::query("SELECT id, name, start_time FROM sessions WHERE record_is_active = 1 ORDER BY id DESC LIMIT 1")
        .fetch_optional(pool).await?;
    Ok(row.map(|r| SimSessionRow {
        id: r.get("id"),
        name: r.try_get("name").ok(),
        start_time: r.try_get("start_time").ok(),
    }))
}

pub async fn sim_insert_session_start(
    pool: &SqlitePool,
    name: &str,
    start_time: i64,
) -> Result<i64> {
    let res =
        sqlx::query("INSERT INTO sessions (name, start_time, record_is_active) VALUES (?1, ?2, 1)")
            .bind(name)
            .bind(start_time)
            .execute(pool)
            .await?;
    Ok(res.last_insert_rowid())
}

pub async fn sim_insert_session_custom(
    pool: &SqlitePool,
    name: &str,
    start_time: i64,
    active: bool,
) -> Result<i64> {
    let res = sqlx::query(
        "INSERT INTO sessions (name, start_time, record_is_active) VALUES (?1, ?2, ?3)",
    )
    .bind(name)
    .bind(start_time)
    .bind(if active { 1 } else { 0 })
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

pub async fn sim_set_active(
    pool: &SqlitePool,
    sid: i64,
    active: bool,
    start_time: Option<i64>,
) -> Result<()> {
    let mut q = sqlx::query("UPDATE sessions SET record_is_active = ?2 WHERE id = ?1")
        .bind(sid)
        .bind(if active { 1 } else { 0 });
    if let Some(ts) = start_time {
        q = sqlx::query("UPDATE sessions SET record_is_active = ?2, start_time = ?3 WHERE id = ?1")
            .bind(sid)
            .bind(if active { 1 } else { 0 })
            .bind(ts);
    }
    q.execute(pool).await?;
    Ok(())
}

/// Name einer Session (aktiv oder beendet), None wenn unbekannt.
pub async fn sim_session_name(pool: &SqlitePool, sid: i64) -> Result<Option<String>> {
    let row = sqlx::query("SELECT name FROM sessions WHERE id = ?1")
        .bind(sid)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.try_get::<Option<String>, _>("name").ok().flatten()))
}

pub async fn sim_session_start_time(pool: &SqlitePool, sid: i64) -> Result<Option<i64>> {
    let row = sqlx::query("SELECT start_time FROM sessions WHERE id = ?1")
        .bind(sid)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get::<i64, _>("start_time")))
}

pub async fn sim_finalize_session(
    pool: &SqlitePool,
    sid: i64,
    stop_time: i64,
    length: i64,
    filename: Option<&str>,
) -> Result<()> {
    let mut q = sqlx::query(
        "UPDATE sessions SET record_is_active = 0, stop_time = ?2, length = ?3, filename = ?4 WHERE id = ?1"
    )
    .bind(sid)
    .bind(stop_time)
    .bind(length);

    // NULL vs. String sauber unterscheiden
    if let Some(f) = filename {
        q = q.bind(f);
    } else {
        q = q.bind::<Option<String>>(None);
    }

    q.execute(pool).await?;
    Ok(())
}

pub async fn sim_delete_session(pool: &SqlitePool, sid: i64) -> Result<()> {
    sqlx::query("DELETE FROM sessions WHERE id = ?1")
        .bind(sid)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn sim_last_finished_session_id(pool: &SqlitePool) -> Result<Option<i64>> {
    let row = sqlx::query("SELECT id FROM sessions WHERE record_is_active = 0 AND filename IS NOT NULL ORDER BY id DESC LIMIT 1")
        .fetch_optional(pool).await?;
    Ok(row.map(|r| r.get::<i64, _>("id")))
}

pub async fn sim_session_filename(pool: &SqlitePool, sid: i64) -> Result<Option<String>> {
    let row = sqlx::query("SELECT filename FROM sessions WHERE id = ?1")
        .bind(sid)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.try_get::<String, _>("filename").ok()))
}

fn app_data_dir() -> PathBuf {
    if let Ok(dir) = env::var("ORS_DATA_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }

    // Neuer Default: systemweiter Datenpfad unter /var/lib/openresquesim
    let var_lib = PathBuf::from("/var/lib/openresquesim");
    if fs::create_dir_all(&var_lib).is_ok() {
        return var_lib;
    } else {
        eprintln!(
            "[db] create_dir_all({}) failed, falling back to user-local paths",
            var_lib.display()
        );
    }

    if let Some(dir) = dirs::data_local_dir() {
        return dir.join("openresquesim");
    }

    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home).join(".local/share/openresquesim");
    }

    // Fallback für Umgebungen ohne HOME/XDG (z.B. systemd ohne User)
    PathBuf::from("/tmp/openresquesim")
}

//Cameras

pub async fn cameras_list(pool: &SqlitePool) -> Result<Vec<Camera>> {
    Ok(
        sqlx::query_as::<_, Camera>("SELECT * FROM cameras ORDER BY created_at DESC")
            .fetch_all(pool)
            .await?,
    )
}

#[derive(Default)]
pub struct CameraPtzFields<'a> {
    pub brand: Option<&'a str>,
    pub ptz_capable: Option<bool>,
    pub vapix_username: Option<&'a str>,
    pub vapix_password: Option<&'a str>,
    pub vapix_port: Option<i64>,
}

pub async fn cameras_add(
    pool: &SqlitePool,
    name: &str,
    url: &str,
    mjpeg_url: Option<&str>,
    fov_deg: Option<f64>,
    ptz: CameraPtzFields<'_>,
) -> Result<i64> {
    let res = sqlx::query(
        "INSERT INTO cameras(name,url,mjpeg_url,fov_deg,brand,ptz_capable,vapix_username,vapix_password,vapix_port) VALUES(?,?,?,?,?,?,?,?,?)",
    )
        .bind(name)
        .bind(url)
        .bind(mjpeg_url)
        .bind(fov_deg)
        .bind(ptz.brand.unwrap_or("generic"))
        .bind(ptz.ptz_capable.unwrap_or(false) as i64)
        .bind(ptz.vapix_username)
        .bind(ptz.vapix_password)
        .bind(ptz.vapix_port)
        .execute(pool)
        .await?;
    Ok(res.last_insert_rowid())
}
pub async fn cameras_update(
    pool: &SqlitePool,
    id: i64,
    name: &str,
    url: &str,
    mjpeg_url: Option<&str>,
    fov_deg: Option<f64>,
    enabled: bool,
    brand: &str,
    ptz_capable: bool,
    vapix_username: Option<&str>,
    vapix_password: Option<&str>,
    vapix_port: Option<i64>,
) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE cameras SET name=?, url=?, mjpeg_url=?, fov_deg=?, enabled=?, brand=?, ptz_capable=?, vapix_username=?, vapix_password=?, vapix_port=? WHERE id=?",
    )
        .bind(name)
        .bind(url)
        .bind(mjpeg_url)
        .bind(fov_deg)
        .bind(if enabled { 1 } else { 0 })
        .bind(brand)
        .bind(ptz_capable as i64)
        .bind(vapix_username)
        .bind(vapix_password)
        .bind(vapix_port)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}
pub async fn cameras_delete(pool: &SqlitePool, id: i64) -> Result<bool> {
    let r = sqlx::query("DELETE FROM cameras WHERE id=?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn camera_get(pool: &SqlitePool, id: i64) -> Result<Camera> {
    let row = sqlx::query_as::<_, Camera>(
        "SELECT id, name, url, mjpeg_url, fov_deg, enabled, created_at, brand, ptz_capable, vapix_username, vapix_password, vapix_port FROM cameras WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .context("get camera by id")?;

    Ok(row)
}

//Record Handling

#[derive(Debug, Serialize, Clone, FromRow)]
pub struct TranscodeJob {
    pub id: i64,
    pub session_id: i64,
    pub input_path: String,
    pub out_dir: String,
    pub mode: String,   // 'mp4' | 'hls' | 'dash'
    pub status: String, // 'pending'|'running'|'done'|'failed'
    pub progress: f64,
    pub log: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

pub async fn job_enqueue(
    pool: &SqlitePool,
    session_id: i64,
    input_path: &str,
    out_dir: &str,
    mode: &str,
) -> Result<i64> {
    let res = sqlx::query(
        "INSERT INTO transcode_jobs(session_id,input_path,out_dir,mode,status) VALUES(?,?,?,?, 'pending')"
    ).bind(session_id).bind(input_path).bind(out_dir).bind(mode)
     .execute(pool).await?;
    Ok(res.last_insert_rowid())
}

pub async fn job_next_pending(pool: &SqlitePool) -> Result<Option<TranscodeJob>> {
    Ok(sqlx::query_as::<_, TranscodeJob>(
        "SELECT * FROM transcode_jobs WHERE status='pending' ORDER BY created_at ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?)
}

pub async fn job_mark_running(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query(
        "UPDATE transcode_jobs SET status='running', started_at=strftime('%s','now') WHERE id=?",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}
pub async fn job_finish_ok(pool: &SqlitePool, id: i64, log: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE transcode_jobs SET status='done', progress=100, log=?, finished_at=strftime('%s','now') WHERE id=?")
        .bind(log).bind(id).execute(pool).await?;
    Ok(())
}
pub async fn job_finish_err(pool: &SqlitePool, id: i64, log: &str) -> Result<()> {
    sqlx::query("UPDATE transcode_jobs SET status='failed', log=?, finished_at=strftime('%s','now') WHERE id=?")
        .bind(log).bind(id).execute(pool).await?;
    Ok(())
}

#[derive(Debug, Serialize, Clone, FromRow)]
pub struct VodItem {
    pub id: i64,
    pub session_id: i64,
    pub kind: String, // 'mp4'|'hls'|'dash'
    pub label: Option<String>,
    pub file_path: Option<String>,
    pub url: Option<String>,
    pub size_bytes: Option<i64>,
    pub duration_sec: Option<f64>,
    pub created_at: i64,
}
pub async fn vod_add(
    pool: &SqlitePool,
    session_id: i64,
    kind: &str,
    label: Option<&str>,
    file_path: Option<&str>,
    url: Option<&str>,
    size_bytes: Option<i64>,
    duration_sec: Option<f64>,
) -> Result<i64> {
    let res = sqlx::query("INSERT INTO vod_items(session_id,kind,label,file_path,url,size_bytes,duration_sec) VALUES(?,?,?,?,?,?,?)")
        .bind(session_id).bind(kind).bind(label).bind(file_path).bind(url).bind(size_bytes).bind(duration_sec)
        .execute(pool).await?;
    Ok(res.last_insert_rowid())
}
pub async fn vod_list_by_session(pool: &SqlitePool, session_id: i64) -> Result<Vec<VodItem>> {
    Ok(
        sqlx::query_as::<_, VodItem>("SELECT * FROM vod_items WHERE session_id=? ORDER BY id ASC")
            .bind(session_id)
            .fetch_all(pool)
            .await?,
    )
}

/// sqlite:///…/app.db
fn app_db_url() -> String {
    let db_path = app_db_path();

    // DB-Datei anlegen, falls nicht vorhanden
    if !db_path.exists() {
        if let Some(parent) = db_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(e) = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&db_path)
        {
            eprintln!("[db] failed to create db file {}: {e}", db_path.display());
        }
    }

    format!("sqlite://{}", db_path.to_string_lossy())
}

fn app_db_path() -> PathBuf {
    if let Ok(path) = env::var("ORS_DB_PATH") {
        if !path.trim().is_empty() {
            let pb = PathBuf::from(&path);
            if let Some(parent) = pb.parent() {
                if fs::create_dir_all(parent).is_ok() {
                    return pb;
                } else {
                    eprintln!(
                        "[db] create_dir_all({}) failed, falling back to tmp",
                        parent.display()
                    );
                }
            }
        }
    }

    let mut dir = app_data_dir();
    if let Err(e) = fs::create_dir_all(&dir) {
        eprintln!("[db] create_dir_all({}) failed: {e}", dir.display());
        dir = PathBuf::from("/tmp/openresquesim");
        let _ = fs::create_dir_all(&dir);
    }

    dir.join("app.db")
}

/// Zero-Arg-Wrapper: nutzt die Standard-URL
pub async fn init_db_default() -> Result<SqlitePool> {
    let db_path = app_db_path();
    let is_new_db = !db_path.exists();
    let url = app_db_url();
    let pool = init_db(&url).await?;

    // Defaults (nur wenn nicht vorhanden):
    let rec_dir_default = app_dirs().rec_dir;
    let _ = fs::create_dir_all(&rec_dir_default);
    settings_ensure(&pool, "record.enabled", "true").await?;
    settings_ensure(&pool, "record.prefix", "rec").await?;
    settings_ensure(&pool, "record.format", "mkv").await?;
    settings_ensure(&pool, "record.segment_secs", "300").await?;
    settings_ensure(&pool, "record.max_files", "0").await?;
    // legacy keys
    settings_ensure(&pool, "record_enabled", "true").await?;
    settings_ensure(&pool, "record_basename", "rec").await?;
    settings_ensure(&pool, "record_container", "mkv").await?;
    settings_ensure(&pool, "record_segment_ms", "300000").await?;
    settings_ensure(&pool, "record_max_files", "0").await?;
    settings_ensure(&pool, "record.postprocess.mode", "none").await?;
    settings_ensure(&pool, "record.postprocess.keep_source", "true").await?;
    settings_ensure(&pool, "record.postprocess.segment_secs", "2").await?;
    settings_ensure(&pool, "record.postprocess.out_dir", "").await?;
    settings_ensure(&pool, "record.dir", &rec_dir_default.to_string_lossy()).await?;
    // legacy key
    settings_ensure(
        &pool,
        "record_directory",
        &rec_dir_default.to_string_lossy(),
    )
    .await?;
    settings_ensure(&pool, "output.rtmp", "").await?;
    settings_ensure(&pool, "audio.device", "").await?;
    // Entfernte Einstellungen (TCP-Ausgang, Live-MP4/HLS) – waren ohne Funktion
    let _ = sqlx::query(
        "DELETE FROM settings WHERE key IN ('output.tcp_port','live_mp4.enabled','live_mp4.dir','live_mp4.segment_ms','live_mp4.max_files')",
    )
    .execute(&pool)
    .await;

    // Setup-Flag: nur bei frischer DB initial auf false setzen.
    if is_new_db {
        let _ = settings_set(&pool, "setup.done", "false").await;
    } else {
        // Bestands-DB soll kein Setup erzwingen.
        let existing = settings_get(&pool, "setup.done").await.ok().flatten();
        if existing.is_none() {
            let _ = settings_set(&pool, "setup.done", "true").await;
        }
    }

    Ok(pool)
}

//Viewers//
#[derive(FromRow, Debug, Clone, serde::Serialize)]
pub struct ViewerRow {
    pub id: i64,
    pub name: String,
    pub alias: Option<String>,
    pub r#type: String,
    pub url: Option<String>,
    pub active: i64,   // 0/1
    pub selected: i64, // 0/1
    pub created_at: i64,
    pub updated_at: Option<i64>,
}

#[derive(serde::Deserialize)]
pub struct ViewerCreate {
    pub name: String,
    pub alias: Option<String>,
    #[serde(default = "default_type")]
    pub r#type: String,
    pub url: Option<String>,
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub selected: Option<bool>,
}
fn default_type() -> String {
    "webrtc".into()
}

#[derive(serde::Deserialize)]
pub struct ViewerPatch {
    pub name: Option<String>,
    pub alias: Option<Option<String>>,
    pub r#type: Option<String>,
    pub url: Option<Option<String>>, // Some(None) zum Nullsetzen
    pub active: Option<bool>,
    pub selected: Option<bool>,
}

// --------------------------------------------------------------
// Rooms
// --------------------------------------------------------------

//Rooms//
#[derive(FromRow, Debug, Clone, serde::Serialize)]
pub struct RoomRow {
    pub id: i64,
    pub name: String,
    pub layout: String,
    pub audio_device: Option<String>,
    pub sources: Option<String>, // JSON: [1, null, 3, ...] → Kamera-IDs
    pub viewer_id: Option<i64>,
    pub speakers: Option<String>, // JSON: [3, 7] → Lautsprecher-IDs (speakers.id)
    pub created_at: i64,
    pub updated_at: Option<i64>,
}

#[derive(serde::Deserialize)]
pub struct RoomCreate {
    pub name: String,
    pub layout: String,
    pub audio_device: Option<String>,
    pub sources: Option<String>, // JSON-String, wie oben
    pub viewer_id: Option<i64>,
    #[serde(default)]
    pub speakers: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct RoomPatch {
    pub name: Option<String>,
    pub layout: Option<String>,
    pub audio_device: Option<Option<String>>, // Some(None) => NULL setzen
    pub sources: Option<Option<String>>,
    pub viewer_id: Option<Option<i64>>,
    pub speakers: Option<Option<String>>,
}

pub async fn rooms_list(db: &Pool<Sqlite>) -> Result<Vec<RoomRow>> {
    sqlx::query_as::<_, RoomRow>(
        r#"
        SELECT id, name, layout, audio_device, sources, viewer_id, speakers, created_at, updated_at
        FROM rooms
        ORDER BY id DESC
        "#,
    )
    .fetch_all(db)
    .await
    .context("list rooms")
}

pub async fn room_get(db: &Pool<Sqlite>, id: i64) -> Result<RoomRow> {
    sqlx::query_as::<_, RoomRow>(
        r#"
        SELECT id, name, layout, audio_device, sources, viewer_id, speakers, created_at, updated_at
        FROM rooms
        WHERE id = ?1
        "#,
    )
    .bind(id)
    .fetch_one(db)
    .await
    .context("get room")
}

pub async fn room_create(db: &Pool<Sqlite>, r: RoomCreate) -> Result<i64> {
    let rec = sqlx::query(
        r#"
        INSERT INTO rooms (name, layout, audio_device, sources, viewer_id, speakers, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%s','now'), strftime('%s','now'))
        "#,
    )
    .bind(&r.name)
    .bind(&r.layout)
    .bind(&r.audio_device)
    .bind(&r.sources)
    .bind(&r.viewer_id)
    .bind(&r.speakers)
    .execute(db)
    .await
    .context("create room")?;

    Ok(rec.last_insert_rowid())
}

pub async fn room_update(db: &Pool<Sqlite>, id: i64, p: RoomPatch) -> Result<()> {
    use std::fmt::Write as _;

    let mut q = String::from("UPDATE rooms SET ");
    let mut first = true;

    macro_rules! set {
        ($frag:expr) => {
            if !first {
                q.push_str(", ");
            }
            q.push_str($frag);
            first = false;
        };
    }

    if p.name.is_some() {
        set!("name = ?");
    }
    if p.layout.is_some() {
        set!("layout = ?");
    }
    if p.audio_device.is_some() {
        set!("audio_device = ?");
    }
    if p.sources.is_some() {
        set!("sources = ?");
    }
    if p.viewer_id.is_some() {
        set!("viewer_id = ?");
    }
    if p.speakers.is_some() {
        set!("speakers = ?");
    }

    set!("updated_at = strftime('%s','now')");
    let _ = first;
    write!(&mut q, " WHERE id = ?").unwrap();

    let mut stmt = sqlx::query(&q);

    if let Some(name) = p.name {
        stmt = stmt.bind(name);
    }
    if let Some(layout) = p.layout {
        stmt = stmt.bind(layout);
    }
    if let Some(ad) = p.audio_device {
        stmt = stmt.bind(ad);
    }
    if let Some(src) = p.sources {
        stmt = stmt.bind(src);
    }
    if let Some(vid) = p.viewer_id {
        stmt = stmt.bind(vid);
    }
    if let Some(sp) = p.speakers {
        stmt = stmt.bind(sp);
    }

    stmt = stmt.bind(id);

    stmt.execute(db).await.context("update room")?;
    Ok(())
}

pub async fn room_delete(db: &Pool<Sqlite>, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM rooms WHERE id = ?1")
        .bind(id)
        .execute(db)
        .await
        .context("delete room")?;
    Ok(())
}
// --------------------------------------------------------------
// Viewers
// --------------------------------------------------------------

pub async fn viewers_list(db: &Pool<Sqlite>) -> Result<Vec<ViewerRow>> {
    sqlx::query_as::<_, ViewerRow>(
        r#"
        SELECT id, name, alias, type, url, active, selected, created_at, updated_at
        FROM viewers ORDER BY id DESC
    "#,
    )
    .fetch_all(db)
    .await
    .context("list viewers")
}

pub async fn viewer_create(db: &Pool<Sqlite>, v: ViewerCreate) -> Result<i64> {
    let active = v.active.unwrap_or(false) as i64;
    let selected = v.selected.unwrap_or(false) as i64;
    let rec = sqlx::query(
        r#"
        INSERT INTO viewers (name, alias, type, url, active, selected, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%s','now'))
    "#,
    )
    .bind(&v.name)
    .bind(&v.alias)
    .bind(&v.r#type)
    .bind(&v.url)
    .bind(active)
    .bind(selected)
    .execute(db)
    .await
    .context("create viewer")?;
    Ok(rec.last_insert_rowid())
}

pub async fn viewer_update(db: &Pool<Sqlite>, id: i64, p: ViewerPatch) -> Result<()> {
    // dynamisches UPDATE
    let mut q = String::from("UPDATE viewers SET ");
    let mut first = true;

    macro_rules! set {
        ($sql:expr) => {{
            if !first {
                q.push_str(", ");
            } else {
                first = false;
            }
            q.push_str($sql);
        }};
    }

    if p.name.is_some() {
        set!("name = ?");
    }
    if p.alias.is_some() {
        set!("alias = ?");
    }
    if p.r#type.is_some() {
        set!("type = ?");
    }
    if p.url.is_some() {
        set!("url = ?");
    }
    if p.active.is_some() {
        set!("active = ?");
    }
    if p.selected.is_some() {
        set!("selected = ?");
    }

    set!("updated_at = strftime('%s','now')");
    let _ = first;
    q.push_str(" WHERE id = ?");

    let mut stmt = sqlx::query(&q);
    if let Some(name) = p.name {
        stmt = stmt.bind(name);
    }
    if let Some(alias) = p.alias {
        stmt = stmt.bind(alias);
    }
    if let Some(t) = p.r#type {
        stmt = stmt.bind(t);
    }
    if let Some(url) = p.url {
        stmt = stmt.bind(url);
    } // Some(None) → NULL
    if let Some(a) = p.active {
        stmt = stmt.bind(a as i64);
    }
    if let Some(s) = p.selected {
        stmt = stmt.bind(s as i64);
    }
    stmt = stmt.bind(id);

    stmt.execute(db).await.context("update viewer")?;
    Ok(())
}

pub async fn viewer_delete(db: &Pool<Sqlite>, id: i64) -> Result<()> {
    sqlx::query(r#"DELETE FROM viewers WHERE id = ?1"#)
        .bind(id)
        .execute(db)
        .await
        .context("delete viewer")?;
    Ok(())
}

pub async fn viewer_get(db: &Pool<Sqlite>, id: i64) -> Result<ViewerRow> {
    sqlx::query_as::<_, ViewerRow>(
        r#"SELECT id, name, alias, type, url, active, selected, created_at, updated_at
           FROM viewers
           WHERE id = ?1
           LIMIT 1"#,
    )
    .bind(id)
    .fetch_one(db)
    .await
    .context("get viewer by id")
}

pub async fn viewer_id_by_url(db: &Pool<Sqlite>, url: &str) -> Result<Option<i64>> {
    let rec = sqlx::query(
        r#"SELECT id FROM viewers WHERE url = ?1 LIMIT 1"#,
    )
    .bind(url)
    .fetch_optional(db)
    .await
    .context("get viewer id by url")?;
    Ok(rec.map(|row| row.get::<i64, _>("id")))
}

fn infer_viewer_type(url: &str) -> &'static str {
    let u = url.to_lowercase();
    if u.starts_with("rtsp://") {
        "rtsp"
    } else if u.ends_with(".mpd") || u.starts_with("http://") || u.starts_with("https://") {
        "dash"
    } else {
        "webrtc"
    }
}

#[derive(FromRow)]
struct ExistingViewer {
    id: i64,
    name: String,
    #[allow(dead_code)]
    r#type: String,
    url: Option<String>,
}

pub async fn viewer_register_broadcast(db: &Pool<Sqlite>, name: &str, url: &str) -> Result<i64> {
    let vtype = infer_viewer_type(url);

    // Lookup per URL
    let existing: Option<ExistingViewer> = sqlx::query_as::<_, ExistingViewer>(
        r#"SELECT id, name, type, url FROM viewers WHERE url = ?1 LIMIT 1"#,
    )
    .bind(url)
    .fetch_optional(db)
    .await
    .context("lookup viewer by url")?;

    if let Some(row) = existing {
        if row.name != name || row.url.as_deref() != Some(url) {
            sqlx::query(
                r#"UPDATE viewers
                   SET name = ?, type = ?, url = ?, updated_at = strftime('%s','now')
                   WHERE id = ?"#,
            )
            .bind(name)
            .bind(vtype)
            .bind(url)
            .bind(row.id)
            .execute(db)
            .await
            .context("update existing viewer from broadcast")?;
        }
        return Ok(row.id);
    }

    // Neu anlegen
    let res = sqlx::query(
        r#"INSERT INTO viewers (name, type, url, active, selected, created_at)
           VALUES (?, ?, ?, 0, 0, strftime('%s','now'))"#,
    )
    .bind(name)
    .bind(vtype)
    .bind(url)
    .execute(db)
    .await
    .context("insert new viewer from broadcast")?;

    Ok(res.last_insert_rowid())
}

// ---- Teilnehmer-Notizblätter ----------------------------------------------

/// Pseudo-Nutzer, unter dem Teilnehmer-Marker in der Marker-Liste erscheinen.
pub const PARTICIPANT_USERNAME: &str = "Teilnehmer";
pub const PARTICIPANT_ROLE: &str = "participant";

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ParticipantSheet {
    pub id: i64,
    pub token: String,
    pub name: String,
    /// Formulardaten als JSON-Objekt (Feldname → Wert)
    pub data: String,
    pub session_id: Option<i64>,
    pub created_by: Option<i64>,
    pub created_at: i64,
    pub updated_at: Option<i64>,
}

const PARTICIPANT_SHEET_COLS: &str =
    "id, token, name, data, session_id, created_by, created_at, updated_at";

pub async fn participant_sheets_list(pool: &SqlitePool) -> Result<Vec<ParticipantSheet>> {
    Ok(sqlx::query_as::<_, ParticipantSheet>(&format!(
        "SELECT {PARTICIPANT_SHEET_COLS} FROM participant_sheets ORDER BY created_at DESC, id DESC"
    ))
    .fetch_all(pool)
    .await?)
}

pub async fn participant_sheet_get(pool: &SqlitePool, id: i64) -> Result<Option<ParticipantSheet>> {
    Ok(sqlx::query_as::<_, ParticipantSheet>(&format!(
        "SELECT {PARTICIPANT_SHEET_COLS} FROM participant_sheets WHERE id = ?1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

pub async fn participant_sheet_by_token(
    pool: &SqlitePool,
    token: &str,
) -> Result<Option<ParticipantSheet>> {
    Ok(sqlx::query_as::<_, ParticipantSheet>(&format!(
        "SELECT {PARTICIPANT_SHEET_COLS} FROM participant_sheets WHERE token = ?1"
    ))
    .bind(token)
    .fetch_optional(pool)
    .await?)
}

/// Legt ein Notizblatt mit zufälligem URL-Token an und gibt die Zeile zurück.
pub async fn participant_sheet_create(
    pool: &SqlitePool,
    name: &str,
    created_by: Option<i64>,
) -> Result<ParticipantSheet> {
    use rand_core::RngCore;
    // Kurzer Token (8 Hex-Zeichen = 4 Byte): deutlich leichter abzutippen/vorzulesen
    // als der frühere 32-stellige Token, bei weiterhin ausreichendem Kollisionsschutz
    // für die realistische Blattanzahl pro Übung. sheet_by_token_or_404() erlaubt
    // weiterhin 8-64 Hex-Zeichen, ältere 32-stellige Tokens bleiben also gültig.
    let mut buf = [0u8; 4];
    for _ in 0..10 {
        rand_core::OsRng.fill_bytes(&mut buf);
        let token: String = buf.iter().map(|b| format!("{b:02x}")).collect();
        let res = sqlx::query(
            "INSERT INTO participant_sheets (token, name, created_by) VALUES (?, ?, ?)",
        )
        .bind(&token)
        .bind(name)
        .bind(created_by)
        .execute(pool)
        .await;
        match res {
            Ok(r) => {
                let id = r.last_insert_rowid();
                return participant_sheet_get(pool, id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("participant sheet insert failed"));
            }
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("failed to generate unique sheet token")
}

pub async fn participant_sheet_set_data(pool: &SqlitePool, id: i64, data_json: &str) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE participant_sheets SET data = ?, updated_at = strftime('%s','now') WHERE id = ?",
    )
    .bind(data_json)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn participant_sheet_set_session(
    pool: &SqlitePool,
    id: i64,
    session_id: Option<i64>,
) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE participant_sheets SET session_id = ?, updated_at = strftime('%s','now') WHERE id = ?",
    )
    .bind(session_id)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn participant_sheet_delete(pool: &SqlitePool, id: i64) -> Result<bool> {
    let r = sqlx::query("DELETE FROM participant_sheets WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

/// Stellt den Pseudo-Nutzer „Teilnehmer“ sicher (ohne Login-Möglichkeit: leerer Hash)
/// und gibt seine ID zurück.
pub async fn ensure_participant_user(pool: &SqlitePool) -> Result<i64> {
    if let Some(u) = user_by_username(pool, PARTICIPANT_USERNAME).await? {
        return Ok(u.id);
    }
    let u = user_insert(pool, PARTICIPANT_USERNAME, "", PARTICIPANT_ROLE).await?;
    Ok(u.id)
}

/// Stellt einen Standard-Marker-Typ mit diesem Namen sicher und gibt seine ID zurück.
pub async fn ensure_marker_type_named(pool: &SqlitePool, name: &str, color: &str) -> Result<i64> {
    let all = marker_types_list(pool, None).await?;
    if let Some(t) = all.iter().find(|t| t.name.eq_ignore_ascii_case(name)) {
        return Ok(t.id);
    }
    let next = marker_types_next_sort_index(pool, None).await.unwrap_or(1);
    marker_types_add(pool, name, color, None, Some(next)).await
}

// ---- Voice of Room: Lautsprecher ------------------------------------------

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct SpeakerRow {
    pub id: i64,
    pub device_id: String,
    pub name: String,
    pub group_name: String,
    pub volume: i64,
    pub muted: i64,
    pub last_ip: Option<String>,
    pub last_seen: Option<i64>,
    pub created_at: i64,
}

const SPEAKER_COLS: &str = "id, device_id, name, group_name, volume, muted, last_ip, last_seen, created_at";

pub async fn speakers_list(pool: &SqlitePool) -> Result<Vec<SpeakerRow>> {
    Ok(sqlx::query_as::<_, SpeakerRow>(&format!(
        "SELECT {SPEAKER_COLS} FROM speakers ORDER BY group_name ASC, name ASC, id ASC"
    ))
    .fetch_all(pool)
    .await?)
}

pub async fn speaker_get(pool: &SqlitePool, id: i64) -> Result<Option<SpeakerRow>> {
    Ok(sqlx::query_as::<_, SpeakerRow>(&format!("SELECT {SPEAKER_COLS} FROM speakers WHERE id = ?1"))
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

pub async fn speaker_by_device(pool: &SqlitePool, device_id: &str) -> Result<Option<SpeakerRow>> {
    Ok(sqlx::query_as::<_, SpeakerRow>(&format!(
        "SELECT {SPEAKER_COLS} FROM speakers WHERE device_id = ?1"
    ))
    .bind(device_id)
    .fetch_optional(pool)
    .await?)
}

/// Legt einen Lautsprecher beim ersten Kontakt an (Name/Gruppe vom Gerät). Existiert er,
/// bleiben die in der UI gepflegten Werte erhalten; nur IP und Zeitstempel werden aktualisiert.
pub async fn speaker_upsert(
    pool: &SqlitePool,
    device_id: &str,
    name: &str,
    group: &str,
    ip: &str,
) -> Result<SpeakerRow> {
    if speaker_by_device(pool, device_id).await?.is_none() {
        let name = name.trim();
        let name = if name.is_empty() { "Lautsprecher" } else { name };
        let group = group.trim();
        let group = if group.is_empty() { "default" } else { group };
        sqlx::query(
            "INSERT INTO speakers (device_id, name, group_name, last_ip, last_seen) VALUES (?, ?, ?, ?, strftime('%s','now'))",
        )
        .bind(device_id)
        .bind(name)
        .bind(group)
        .bind(ip)
        .execute(pool)
        .await?;
    } else {
        sqlx::query("UPDATE speakers SET last_ip = ?, last_seen = strftime('%s','now') WHERE device_id = ?")
            .bind(ip)
            .bind(device_id)
            .execute(pool)
            .await?;
    }
    speaker_by_device(pool, device_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("speaker upsert failed"))
}

pub async fn speaker_touch(pool: &SqlitePool, id: i64, ip: &str) -> Result<()> {
    sqlx::query("UPDATE speakers SET last_ip = ?, last_seen = strftime('%s','now') WHERE id = ?")
        .bind(ip)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn speaker_update(
    pool: &SqlitePool,
    id: i64,
    name: &str,
    group: &str,
    volume: i64,
    muted: bool,
) -> Result<bool> {
    let r = sqlx::query("UPDATE speakers SET name = ?, group_name = ?, volume = ?, muted = ? WHERE id = ?")
        .bind(name)
        .bind(group)
        .bind(volume)
        .bind(if muted { 1 } else { 0 })
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn speaker_delete(pool: &SqlitePool, id: i64) -> Result<bool> {
    let r = sqlx::query("DELETE FROM speakers WHERE id = ?").bind(id).execute(pool).await?;
    Ok(r.rows_affected() > 0)
}
