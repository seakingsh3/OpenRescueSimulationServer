//! Kopplung von Viewer-Bildschirmen und Pagern (wie Bluetooth-Koppeln).
//!
//! Ein Gerät, das `/viewer/<n>` bzw. `/pager/<n>` öffnet, bekommt erst dann Bild, Ton und
//! Tickets, wenn es gekoppelt ist. Koppeln:
//! - **QR-Code**: Der Zuschauer-QR-Assistent (bzw. „Öffnen“ in der Liveansicht) erzeugt einen
//!   Link `/viewer/<n>?pair=<code>`; das Gerät koppelt sich beim Öffnen selbst.
//! - **PIN**: In den Einstellungen „Gerät koppeln“ → 6-stellige PIN, am Gerät eintippen.
//!
//! Codes und PINs gelten 2 Minuten und nur einmal. Das Gerät erhält einen zufälligen
//! Geräteschlüssel (32 Byte), den es im Browser speichert; der Server kennt nur dessen
//! SHA-256-Hash. Viewer und Pager mit derselben Nummer teilen sich den Steuerkanal und
//! damit die Kopplung.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use once_cell::sync::Lazy;
use rand_core::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::routes::AuthUser;
use crate::state::AppCtx;

const CODE_VALIDITY: Duration = Duration::from_secs(120);
/// Nach so vielen falschen PIN-Eingaben werden alle offenen PINs verworfen (Raten verhindern)
const MAX_FAILED_CLAIMS: u32 = 10;

struct PendingCode {
    /// QR-Codes gehören zu einer festen Nummer, PINs gelten für die Nummer des Geräts
    viewer_no: Option<i64>,
    expires: Instant,
    created_by: String,
    is_pin: bool,
}

struct Pending {
    codes: HashMap<String, PendingCode>,
    failed: u32,
}

static PENDING: Lazy<Mutex<Pending>> =
    Lazy::new(|| Mutex::new(Pending { codes: HashMap::new(), failed: 0 }));

pub async fn init_db(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS device_pairings(
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            viewer_no  INTEGER NOT NULL,          -- Nummer aus /viewer/<n> bzw. /pager/<n>
            key_hash   TEXT    NOT NULL UNIQUE,   -- SHA-256 des Geräteschlüssels (hex)
            label      TEXT    NULL,              -- z.B. Browser/Gerät, frei änderbar
            created_by TEXT    NULL,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            last_seen  INTEGER NULL
        )"#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS speaker_keys(
            speaker_id  INTEGER PRIMARY KEY,       -- speakers.id
            key_hash    TEXT    NOT NULL,          -- SHA-256 des Geräteschlüssels (hex)
            approved_by TEXT    NULL,
            approved_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
        )"#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- Raumlautsprecher
// ESP-Lautsprecher haben weder Bildschirm noch Tastatur: Ein neues Gerät erscheint als
// „nicht freigegeben“ und bekommt keinen Ton. Nach dem Piepton (Identifizieren) gibt man es
// in der Lautsprecherliste frei; der Server schickt ihm einen Schlüssel, den es dauerhaft
// speichert und bei jeder Anmeldung mitschickt.

pub async fn speaker_key_hash(pool: &SqlitePool, speaker_id: i64) -> Option<String> {
    sqlx::query_scalar("SELECT key_hash FROM speaker_keys WHERE speaker_id = ?1")
        .bind(speaker_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

/// Neuen Schlüssel für einen Lautsprecher erzeugen und speichern; liefert den Klartext.
pub async fn speaker_key_create(pool: &SqlitePool, speaker_id: i64, by: &str) -> anyhow::Result<String> {
    let key = random_hex(32);
    sqlx::query(
        "INSERT INTO speaker_keys (speaker_id, key_hash, approved_by) VALUES (?1, ?2, ?3)
         ON CONFLICT(speaker_id) DO UPDATE SET key_hash = excluded.key_hash, approved_by = excluded.approved_by, approved_at = strftime('%s','now')",
    )
    .bind(speaker_id)
    .bind(hash_key(&key))
    .bind(by)
    .execute(pool)
    .await?;
    Ok(key)
}

pub async fn speaker_key_delete(pool: &SqlitePool, speaker_id: i64) {
    let _ = sqlx::query("DELETE FROM speaker_keys WHERE speaker_id = ?1")
        .bind(speaker_id)
        .execute(pool)
        .await;
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand_core::OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn random_pin() -> String {
    let mut buf = [0u8; 4];
    rand_core::OsRng.fill_bytes(&mut buf);
    format!("{:06}", u32::from_le_bytes(buf) % 1_000_000)
}

pub fn hash_key(key: &str) -> String {
    let digest = Sha256::digest(key.trim().as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Ist dieser Geräteschlüssel für die Viewer-Nummer gekoppelt? Aktualisiert „zuletzt gesehen“.
pub async fn device_allowed(pool: &SqlitePool, viewer_no: i64, device_key: Option<&str>) -> bool {
    let Some(key) = device_key.map(str::trim).filter(|k| !k.is_empty()) else {
        return false;
    };
    let hash = hash_key(key);
    let found: Option<i64> =
        sqlx::query_scalar("SELECT id FROM device_pairings WHERE viewer_no = ?1 AND key_hash = ?2")
            .bind(viewer_no)
            .bind(&hash)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    if let Some(id) = found {
        let _ = sqlx::query("UPDATE device_pairings SET last_seen = strftime('%s','now') WHERE id = ?1")
            .bind(id)
            .execute(pool)
            .await;
        true
    } else {
        false
    }
}

/// Kopplung starten (eingeloggt): liefert eine PIN und – wenn eine Nummer angegeben ist –
/// einen QR-Link. Beide gelten 2 Minuten und nur einmal.
#[derive(Deserialize, Default)]
pub struct StartPayload {
    pub viewer_no: Option<i64>,
}

pub async fn pairing_start(
    user: AuthUser,
    Json(body): Json<StartPayload>,
) -> Json<Value> {
    let mut p = PENDING.lock().await;
    let now = Instant::now();
    p.codes.retain(|_, c| c.expires > now);
    let pin = loop {
        let pin = random_pin();
        if !p.codes.contains_key(&pin) {
            break pin;
        }
    };
    p.codes.insert(
        pin.clone(),
        PendingCode { viewer_no: None, expires: now + CODE_VALIDITY, created_by: user.username.clone(), is_pin: true },
    );
    let mut out = json!({ "pin": pin, "expires_in": CODE_VALIDITY.as_secs() });
    if let Some(n) = body.viewer_no.filter(|n| *n > 0) {
        let code = random_hex(16);
        p.codes.insert(
            code.clone(),
            PendingCode { viewer_no: Some(n), expires: now + CODE_VALIDITY, created_by: user.username.clone(), is_pin: false },
        );
        out["code"] = json!(code);
        out["link"] = json!(format!("/viewer/{n}?pair={code}"));
    }
    tracing::info!(user = %user.username, viewer_no = ?body.viewer_no, "🔗 Kopplung gestartet");
    Json(out)
}

#[derive(Deserialize)]
pub struct ClaimPayload {
    /// PIN oder QR-Code
    pub code: String,
    pub viewer_no: i64,
    #[serde(default)]
    pub label: Option<String>,
}

/// Kopplung abschließen (öffentlich, vom Gerät aus): gültige PIN/QR-Code → Geräteschlüssel.
pub async fn pairing_claim(
    State(ctx): State<AppCtx>,
    Json(body): Json<ClaimPayload>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let code = body.code.trim().to_string();
    let entry = {
        let mut p = PENDING.lock().await;
        let now = Instant::now();
        p.codes.retain(|_, c| c.expires > now);
        let ok = p
            .codes
            .get(&code)
            .map(|c| c.viewer_no.map_or(true, |n| n == body.viewer_no))
            .unwrap_or(false);
        if ok {
            p.codes.remove(&code)
        } else {
            p.failed += 1;
            if p.failed >= MAX_FAILED_CLAIMS {
                // Zu viele Fehlversuche: offene PINs verwerfen, damit niemand alle 10^6 durchprobiert
                p.codes.retain(|_, c| !c.is_pin);
                p.failed = 0;
                tracing::warn!("🔗 Kopplung: zu viele falsche PINs – offene PINs verworfen");
            }
            None
        }
    };
    let Some(entry) = entry else {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Code ungültig oder abgelaufen" })),
        ));
    };
    let key = random_hex(32);
    let label = body.label.map(|l| l.chars().take(80).collect::<String>());
    sqlx::query("INSERT INTO device_pairings (viewer_no, key_hash, label, created_by) VALUES (?1, ?2, ?3, ?4)")
        .bind(body.viewer_no)
        .bind(hash_key(&key))
        .bind(&label)
        .bind(&entry.created_by)
        .execute(&ctx.db)
        .await
        .map_err(|e| {
            tracing::error!(?e, "pairing insert");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "db" })))
        })?;
    PENDING.lock().await.failed = 0;
    tracing::info!(viewer_no = body.viewer_no, by = %entry.created_by, "🔗 Gerät gekoppelt");
    crate::logging::emit_ui_event(json!({ "type": "device_paired", "viewer_no": body.viewer_no }));
    Ok(Json(json!({ "device_key": key, "viewer_no": body.viewer_no })))
}

/// Gekoppelte Geräte (eingeloggt)
pub async fn pairings_list(_user: AuthUser, State(ctx): State<AppCtx>) -> Result<Json<Value>, StatusCode> {
    let rows: Vec<(i64, i64, Option<String>, Option<String>, i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, viewer_no, label, created_by, created_at, last_seen FROM device_pairings ORDER BY viewer_no, id",
    )
    .fetch_all(&ctx.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let list: Vec<Value> = rows
        .into_iter()
        .map(|(id, viewer_no, label, created_by, created_at, last_seen)| {
            json!({ "id": id, "viewer_no": viewer_no, "label": label, "created_by": created_by, "created_at": created_at, "last_seen": last_seen })
        })
        .collect();
    Ok(Json(json!({ "pairings": list })))
}

/// Gerät entkoppeln (eingeloggt). Offene Verbindungen dieser Nummer werden getrennt; noch
/// gekoppelte Geräte derselben Nummer verbinden sich sofort wieder.
pub async fn pairing_delete(
    _user: AuthUser,
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    let viewer_no: Option<i64> = sqlx::query_scalar("SELECT viewer_no FROM device_pairings WHERE id = ?1")
        .bind(id)
        .fetch_optional(&ctx.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let Some(viewer_no) = viewer_no else { return Err(StatusCode::NOT_FOUND) };
    sqlx::query("DELETE FROM device_pairings WHERE id = ?1")
        .bind(id)
        .execute(&ctx.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    disconnect_viewer_no(&ctx, viewer_no).await;
    tracing::info!(viewer_no, "🔗 Gerät entkoppelt");
    Ok(Json(json!({ "ok": true })))
}

/// Alle Steuerverbindungen einer Viewer-Nummer trennen (Kanal schließen → Seite verbindet neu
/// und muss sich dabei ausweisen).
pub async fn disconnect_viewer_no(ctx: &AppCtx, viewer_no: i64) {
    let url = format!("/viewer/{viewer_no}");
    let db_id: Option<i64> = sqlx::query_scalar("SELECT id FROM viewers WHERE url = ?1")
        .bind(&url)
        .fetch_optional(&ctx.db)
        .await
        .ok()
        .flatten();
    if let Some(db_id) = db_id {
        if let Some(list) = ctx.viewer_channels.lock().await.remove(&db_id) {
            for tx in list {
                let _ = tx.send(json!({ "cmd": "pair_required" }).to_string());
            }
        }
    }
}
