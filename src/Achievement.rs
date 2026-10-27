use axum::http::HeaderMap;
use chrono::{Local, Timelike, Utc};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::json;
use sqlx::{QueryBuilder, Row, SqlitePool};

use crate::state::AppCtx;

const COUNT_THRESHOLDS: &[i64] = &[25, 50, 100, 200, 300, 400, 500, 1000];
const SHORT_SIM_MS: i64 = 5 * 60 * 1000;
const LONG_SIM_MS: i64 = 2 * 60 * 60 * 1000;
const MAX_SIM_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug)]
struct AchievementDef {
    key: String,
    name: String,
    description: String,
    points: i64,
    hidden: bool,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JwtClaims {
    sub: String,
    role: String,
    uid: i64,
    exp: usize,
}

pub fn auth_user_id_from_headers(ctx: &AppCtx, headers: &HeaderMap) -> Option<i64> {
    let auth = headers.get(axum::http::header::AUTHORIZATION)?;
    let auth_str = auth.to_str().ok()?;
    let prefix = "bearer ";
    let token = if auth_str.to_ascii_lowercase().starts_with(prefix) {
        auth_str[prefix.len()..].trim()
    } else {
        return None;
    };
    let data = decode::<JwtClaims>(
        token,
        &DecodingKey::from_secret(ctx.auth_secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )
    .ok()?;
    Some(data.claims.uid)
}

pub async fn seed_defs(pool: &SqlitePool) -> sqlx::Result<()> {
    for def in achievement_defs() {
        sqlx::query(
            r#"
            INSERT INTO achievements (key, name, description, points, hidden)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(key) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                points = excluded.points,
                hidden = excluded.hidden
            "#,
        )
        .bind(def.key)
        .bind(def.name)
        .bind(def.description)
        .bind(def.points)
        .bind(if def.hidden { 1 } else { 0 })
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn on_marker_created(
    pool: &SqlitePool,
    user_id: i64,
    record_time_sec: i64,
    has_text: bool,
) -> sqlx::Result<Vec<String>> {
    ensure_user_stats(pool, user_id).await?;
    let now = Utc::now().timestamp();
    let text_inc = if has_text { 1 } else { 0 };
    sqlx::query(
        r#"
        UPDATE user_stats
        SET markers_total = markers_total + 1,
            markers_text_total = markers_text_total + ?2,
            last_marker_at = ?3
        WHERE user_id = ?1
        "#,
    )
    .bind(user_id)
    .bind(text_inc)
    .bind(now)
    .execute(pool)
    .await?;

    let (markers_total, markers_text_total) = marker_counts(pool, user_id).await?;

    let mut unlocked = Vec::new();
    if markers_total == 1 {
        maybe_unlock(pool, user_id, "marker_first", None, &mut unlocked).await?;
    }

    for t in COUNT_THRESHOLDS {
        if markers_total >= *t {
            let key = format!("markers_{}", t);
            maybe_unlock(pool, user_id, &key, None, &mut unlocked).await?;
        }
    }

    if markers_text_total >= 10 {
        maybe_unlock(pool, user_id, "marker_text_10", None, &mut unlocked).await?;
    }
    if markers_text_total >= 50 {
        maybe_unlock(pool, user_id, "marker_text_50", None, &mut unlocked).await?;
    }

    if record_time_sec <= 60 {
        maybe_unlock(pool, user_id, "marker_quickdraw", None, &mut unlocked).await?;
    }

    let hour = Local::now().hour();
    if (2..=4).contains(&hour) {
        maybe_unlock(pool, user_id, "marker_night_owl", None, &mut unlocked).await?;
    }
    if (6..=8).contains(&hour) {
        maybe_unlock(pool, user_id, "marker_early_bird", None, &mut unlocked).await?;
    }

    Ok(unlocked)
}

pub async fn on_sim_started(pool: &SqlitePool, user_id: i64, started_at: i64) -> sqlx::Result<Vec<String>> {
    ensure_user_stats(pool, user_id).await?;
    sqlx::query(
        r#"
        UPDATE user_stats
        SET sim_starts_total = sim_starts_total + 1,
            last_sim_start_at = ?2
        WHERE user_id = ?1
        "#,
    )
    .bind(user_id)
    .bind(started_at)
    .execute(pool)
    .await?;

    let sim_starts_total = sim_start_count(pool, user_id).await?;
    let mut unlocked = Vec::new();
    if sim_starts_total == 1 {
        maybe_unlock(pool, user_id, "simstart_first", None, &mut unlocked).await?;
    }
    for t in COUNT_THRESHOLDS {
        if sim_starts_total >= *t {
            let key = format!("simstart_{}", t);
            maybe_unlock(pool, user_id, &key, None, &mut unlocked).await?;
        }
    }

    let hour = Local::now().hour();
    if (0..=3).contains(&hour) {
        maybe_unlock(pool, user_id, "simstart_night_shift", None, &mut unlocked).await?;
    }
    if (5..=7).contains(&hour) {
        maybe_unlock(pool, user_id, "simstart_early_bird", None, &mut unlocked).await?;
    }

    Ok(unlocked)
}

pub async fn on_sim_stopped(pool: &SqlitePool, user_id: i64, length_ms: i64) -> sqlx::Result<Vec<String>> {
    ensure_user_stats(pool, user_id).await?;
    let now = Utc::now().timestamp();
    sqlx::query(
        r#"
        UPDATE user_stats
        SET last_sim_stop_at = ?2,
            last_sim_length_ms = ?3
        WHERE user_id = ?1
        "#,
    )
    .bind(user_id)
    .bind(now)
    .bind(length_ms)
    .execute(pool)
    .await?;

    let mut unlocked = Vec::new();
    if length_ms > MAX_SIM_MS {
        tracing::warn!(
            user_id,
            length_ms,
            "sim length > 24h, skipping length achievements"
        );
        return Ok(unlocked);
    }

    if length_ms <= SHORT_SIM_MS {
        let ctx = json!({ "length_ms": length_ms, "kind": "short" });
        maybe_unlock(pool, user_id, "sim_short", Some(ctx), &mut unlocked).await?;
    }
    if length_ms >= LONG_SIM_MS {
        let ctx = json!({ "length_ms": length_ms, "kind": "long" });
        maybe_unlock(pool, user_id, "sim_long", Some(ctx), &mut unlocked).await?;
    }

    Ok(unlocked)
}

pub async fn on_debrief_started(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<String>> {
    ensure_user_stats(pool, user_id).await?;
    let now = Utc::now().timestamp();
    sqlx::query(
        r#"
        UPDATE user_stats
        SET debriefs_total = debriefs_total + 1,
            last_debrief_at = ?2
        WHERE user_id = ?1
        "#,
    )
    .bind(user_id)
    .bind(now)
    .execute(pool)
    .await?;

    let debriefs_total = debrief_count(pool, user_id).await?;
    let mut unlocked = Vec::new();
    if debriefs_total == 1 {
        maybe_unlock(pool, user_id, "debrief_first", None, &mut unlocked).await?;
    }
    for t in COUNT_THRESHOLDS {
        if debriefs_total >= *t {
            let key = format!("debrief_{}", t);
            maybe_unlock(pool, user_id, &key, None, &mut unlocked).await?;
        }
    }

    Ok(unlocked)
}

#[derive(Debug, serde::Serialize)]
pub struct UserStatsRow {
    pub user_id: i64,
    pub markers_total: i64,
    pub markers_text_total: i64,
    pub sim_starts_total: i64,
    pub debriefs_total: i64,
    pub last_marker_at: Option<i64>,
    pub last_sim_start_at: Option<i64>,
    pub last_sim_stop_at: Option<i64>,
    pub last_sim_length_ms: Option<i64>,
    pub last_debrief_at: Option<i64>,
}

#[derive(Debug, serde::Serialize)]
pub struct AchievementRow {
    pub key: String,
    pub name: String,
    pub description: String,
    pub points: i64,
    pub hidden: bool,
    pub unlocked_at: Option<i64>,
    pub context: Option<String>,
}

pub async fn get_user_stats(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Option<UserStatsRow>> {
    let row = sqlx::query(
        r#"
        SELECT user_id, markers_total, markers_text_total, sim_starts_total, debriefs_total,
               last_marker_at, last_sim_start_at, last_sim_stop_at, last_sim_length_ms, last_debrief_at
        FROM user_stats WHERE user_id = ?1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| UserStatsRow {
        user_id: r.get("user_id"),
        markers_total: r.get("markers_total"),
        markers_text_total: r.get("markers_text_total"),
        sim_starts_total: r.get("sim_starts_total"),
        debriefs_total: r.get("debriefs_total"),
        last_marker_at: r.try_get("last_marker_at").ok(),
        last_sim_start_at: r.try_get("last_sim_start_at").ok(),
        last_sim_stop_at: r.try_get("last_sim_stop_at").ok(),
        last_sim_length_ms: r.try_get("last_sim_length_ms").ok(),
        last_debrief_at: r.try_get("last_debrief_at").ok(),
    }))
}

pub async fn list_user_achievements(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<AchievementRow>> {
    let rows = sqlx::query(
        r#"
        SELECT a.key, a.name, a.description, a.points, a.hidden,
               ua.unlocked_at, ua.context
        FROM achievements a
        LEFT JOIN user_achievements ua
          ON ua.achievement_key = a.key AND ua.user_id = ?1
        ORDER BY a.key ASC
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| AchievementRow {
            key: r.get("key"),
            name: r.get("name"),
            description: r.get("description"),
            points: r.get("points"),
            hidden: r.get::<i64, _>("hidden") != 0,
            unlocked_at: r.try_get("unlocked_at").ok(),
            context: r.try_get("context").ok(),
        })
        .collect())
}


pub async fn list_all_user_stats_paginated(
    pool: &SqlitePool,
    limit: i64,
    offset: i64,
) -> sqlx::Result<(Vec<(i64, String, String, UserStatsRow)>, i64)> {
    let total_row = sqlx::query("SELECT COUNT(*) as cnt FROM users")
        .fetch_one(pool)
        .await?;
    let total: i64 = total_row.get("cnt");

    let rows = sqlx::query(
        r#"
        SELECT u.id as user_id, u.username, u.role,
               s.markers_total, s.markers_text_total, s.sim_starts_total, s.debriefs_total,
               s.last_marker_at, s.last_sim_start_at, s.last_sim_stop_at, s.last_sim_length_ms, s.last_debrief_at
        FROM users u
        LEFT JOIN user_stats s ON s.user_id = u.id
        ORDER BY u.username ASC
        LIMIT ?1 OFFSET ?2
        "#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    let out = rows
        .into_iter()
        .map(|r| {
            let stats = UserStatsRow {
                user_id: r.get("user_id"),
                markers_total: r.try_get::<i64, _>("markers_total").unwrap_or(0),
                markers_text_total: r.try_get::<i64, _>("markers_text_total").unwrap_or(0),
                sim_starts_total: r.try_get::<i64, _>("sim_starts_total").unwrap_or(0),
                debriefs_total: r.try_get::<i64, _>("debriefs_total").unwrap_or(0),
                last_marker_at: r.try_get("last_marker_at").ok(),
                last_sim_start_at: r.try_get("last_sim_start_at").ok(),
                last_sim_stop_at: r.try_get("last_sim_stop_at").ok(),
                last_sim_length_ms: r.try_get("last_sim_length_ms").ok(),
                last_debrief_at: r.try_get("last_debrief_at").ok(),
            };
            (r.get("user_id"), r.get("username"), r.get("role"), stats)
        })
        .collect();

    Ok((out, total))
}

pub async fn achievement_payloads_by_keys(
    pool: &SqlitePool,
    keys: &[String],
) -> sqlx::Result<Vec<AchievementRow>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb = QueryBuilder::new(
        "SELECT key, name, description, points, hidden FROM achievements WHERE key IN (",
    );
    let mut separated = qb.separated(", ");
    for k in keys {
        separated.push_bind(k);
    }
    qb.push(") ORDER BY key ASC");
    let rows = qb.build().fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| AchievementRow {
            key: r.get("key"),
            name: r.get("name"),
            description: r.get("description"),
            points: r.get("points"),
            hidden: r.get::<i64, _>("hidden") != 0,
            unlocked_at: None,
            context: None,
        })
        .collect())
}

async fn ensure_user_stats(pool: &SqlitePool, user_id: i64) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO user_stats (user_id)
        VALUES (?1)
        ON CONFLICT(user_id) DO NOTHING
        "#,
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn marker_counts(pool: &SqlitePool, user_id: i64) -> sqlx::Result<(i64, i64)> {
    let row = sqlx::query(
        "SELECT markers_total, markers_text_total FROM user_stats WHERE user_id = ?1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok((row.get::<i64, _>("markers_total"), row.get::<i64, _>("markers_text_total")))
}

async fn sim_start_count(pool: &SqlitePool, user_id: i64) -> sqlx::Result<i64> {
    let row = sqlx::query("SELECT sim_starts_total FROM user_stats WHERE user_id = ?1")
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    Ok(row.get::<i64, _>("sim_starts_total"))
}

async fn debrief_count(pool: &SqlitePool, user_id: i64) -> sqlx::Result<i64> {
    let row = sqlx::query("SELECT debriefs_total FROM user_stats WHERE user_id = ?1")
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    Ok(row.get::<i64, _>("debriefs_total"))
}

async fn maybe_unlock(
    pool: &SqlitePool,
    user_id: i64,
    key: &str,
    context: Option<serde_json::Value>,
    unlocked: &mut Vec<String>,
) -> sqlx::Result<()> {
    let ctx = context.map(|v| v.to_string());
    let res = sqlx::query(
        r#"
        INSERT OR IGNORE INTO user_achievements (user_id, achievement_key, context)
        VALUES (?1, ?2, ?3)
        "#,
    )
    .bind(user_id)
    .bind(key)
    .bind(ctx)
    .execute(pool)
    .await?;
    if res.rows_affected() > 0 {
        tracing::info!(user_id, key, "achievement unlocked");
        unlocked.push(key.to_string());
    }
    Ok(())
}

fn achievement_defs() -> Vec<AchievementDef> {
    let mut defs = Vec::new();

    defs.push(AchievementDef {
        key: "marker_first".to_string(),
        name: "Erster Marker".to_string(),
        description: "Dein allererster Marker.".to_string(),
        points: 5,
        hidden: false,
    });
    defs.push(AchievementDef {
        key: "simstart_first".to_string(),
        name: "Erster Start".to_string(),
        description: "Deine erste gestartete Simulation.".to_string(),
        points: 5,
        hidden: false,
    });
    defs.push(AchievementDef {
        key: "debrief_first".to_string(),
        name: "Erstes Debriefing".to_string(),
        description: "Debriefing zum ersten Mal geöffnet.".to_string(),
        points: 5,
        hidden: false,
    });

    for t in COUNT_THRESHOLDS {
        defs.push(AchievementDef {
            key: format!("markers_{}", t),
            name: format!("Marker {t}"),
            description: format!("{t} Marker gesetzt."),
            points: t / 5,
            hidden: false,
        });
        defs.push(AchievementDef {
            key: format!("simstart_{}", t),
            name: format!("Simulationen {t}"),
            description: format!("{t} Simulationen gestartet."),
            points: t / 5,
            hidden: false,
        });
        defs.push(AchievementDef {
            key: format!("debrief_{}", t),
            name: format!("Debriefings {t}"),
            description: format!("{t} Debriefings durchgeführt."),
            points: t / 5,
            hidden: false,
        });
    }

    defs.push(AchievementDef {
        key: "sim_short".to_string(),
        name: "Blitzrunde".to_string(),
        description: "Simulation besonders kurz (≤ 5 Minuten).".to_string(),
        points: 15,
        hidden: false,
    });
    defs.push(AchievementDef {
        key: "sim_long".to_string(),
        name: "Marathon".to_string(),
        description: "Simulation besonders lang (≥ 2 Stunden).".to_string(),
        points: 20,
        hidden: false,
    });

    defs.push(AchievementDef {
        key: "marker_text_10".to_string(),
        name: "Kommentator I".to_string(),
        description: "10 Marker mit Text versehen.".to_string(),
        points: 10,
        hidden: false,
    });
    defs.push(AchievementDef {
        key: "marker_text_50".to_string(),
        name: "Kommentator II".to_string(),
        description: "50 Marker mit Text versehen.".to_string(),
        points: 20,
        hidden: false,
    });
    defs.push(AchievementDef {
        key: "marker_quickdraw".to_string(),
        name: "Schnellschuss".to_string(),
        description: "Marker innerhalb der ersten Minute gesetzt.".to_string(),
        points: 10,
        hidden: false,
    });
    defs.push(AchievementDef {
        key: "marker_night_owl".to_string(),
        name: "Nachteule".to_string(),
        description: "Marker zwischen 02:00–04:59 gesetzt.".to_string(),
        points: 10,
        hidden: true,
    });
    defs.push(AchievementDef {
        key: "marker_early_bird".to_string(),
        name: "Frühaufsteher".to_string(),
        description: "Marker zwischen 06:00–08:59 gesetzt.".to_string(),
        points: 10,
        hidden: true,
    });
    defs.push(AchievementDef {
        key: "simstart_night_shift".to_string(),
        name: "Nachtschicht".to_string(),
        description: "Simulation zwischen 00:00–03:59 gestartet.".to_string(),
        points: 10,
        hidden: true,
    });
    defs.push(AchievementDef {
        key: "simstart_early_bird".to_string(),
        name: "Frühstarter".to_string(),
        description: "Simulation zwischen 05:00–07:59 gestartet.".to_string(),
        points: 10,
        hidden: true,
    });

    defs
}
