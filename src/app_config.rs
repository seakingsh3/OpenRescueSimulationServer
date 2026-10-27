use sqlx::SqlitePool;

use crate::db::settings_get;

fn truthy(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "y" | "on"
    )
}

/* ------------------------ Low-level Helfer ------------------------ */

pub async fn get_opt(pool: &SqlitePool, key: &str) -> Option<String> {
    settings_get(pool, key).await.ok().flatten()
}

pub async fn get_str_or(pool: &SqlitePool, key: &str, default: &str) -> String {
    get_opt(pool, key)
        .await
        .unwrap_or_else(|| default.to_string())
}

pub async fn get_bool_or(pool: &SqlitePool, key: &str, default: bool) -> bool {
    match get_opt(pool, key).await {
        Some(v) => truthy(&v),
        None => default,
    }
}

pub async fn get_u32_or(pool: &SqlitePool, key: &str, default: u32) -> u32 {
    get_opt(pool, key)
        .await
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(default)
}

pub async fn get_i32_or(pool: &SqlitePool, key: &str, default: i32) -> i32 {
    get_opt(pool, key)
        .await
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(default)
}

/* ------------------------ High-level Settings ------------------------ */
/* Output / Audio */

pub async fn audio_device(pool: &SqlitePool) -> Option<String> {
    get_opt(pool, "audio.device").await
}

/* Recording (einheitlich für alle Call-Sites) */

pub async fn record_enabled(pool: &SqlitePool) -> bool {
    get_bool_or(pool, "record.enabled", false).await
}

/// Container-String ("mp4" | "mkv" | "mov"), Default "mp4"
pub async fn record_container(pool: &SqlitePool) -> String {
    get_opt(pool, "record.format")
        .await
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| s == "mp4" || s == "mkv" || s == "mov")
        .unwrap_or_else(|| "mkv".to_string())
}

pub async fn record_dir(pool: &SqlitePool, default_dir: &str) -> String {
    get_str_or(pool, "record.dir", default_dir).await
}

pub async fn record_prefix(pool: &SqlitePool, default_prefix: &str) -> String {
    get_str_or(pool, "record.prefix", default_prefix).await
}

/// Segmentlänge in Sekunden (UI/Setting), Default `default_secs`
pub async fn record_segment_secs(pool: &SqlitePool, default_secs: u32) -> u32 {
    get_u32_or(pool, "record.segment_secs", default_secs).await
}

/// Optional: maximale Anzahl an Segmentdateien (0 = unbegrenzt)
pub async fn record_max_files(pool: &SqlitePool) -> i32 {
    get_i32_or(pool, "record.max_files", 0).await
}
