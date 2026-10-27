//! Zugang zu den Live-WebSockets (Video, Durchsage, Logs).
//!
//! Browser können bei WebSockets keinen Authorization-Header setzen, daher kommt der
//! Nachweis als Query-Parameter:
//! - `?token=<JWT>`  – eingeloggte Oberfläche (Liveansicht, PTT, Systemstatus/Logs)
//! - `?ticket=<…>`   – Zuschauer-Bildschirme und Pager ohne Login. Der Server legt das
//!   Ticket den Befehlen `show_live` bzw. `paging_start` bei, die nur eingeloggte Nutzer
//!   (Session-Start, Durchsage) auslösen. Ein Video-Ticket gilt nur für die Session, für
//!   die es ausgestellt wurde, ein Durchsage-Ticket nur für die laufende Durchsage.
//!
//! Früher waren /ws/webrtc, /ws/voice/* und /ws/logs ohne jeden Nachweis erreichbar: Wer
//! den Server im Netz erreichte, konnte das Live-Bild ansehen, in die Raumlautsprecher
//! sprechen und die Logs (inkl. Verteiler-Streamschlüssel) mitlesen.

use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::routes::AuthUser;
use crate::state::AppCtx;

#[derive(Debug, Default, Deserialize)]
pub struct WsAuthQuery {
    pub token: Option<String>,
    pub ticket: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct TicketClaims {
    /// "video" oder "paging"
    scope: String,
    /// Video: Session-ID; Durchsage: laufende Nummer der Durchsage
    id: i64,
    exp: usize,
}

/// Eingeloggter Nutzer aus `?token=` (gleiche Prüfung wie der Authorization-Header).
pub fn user_from_query(ctx: &AppCtx, q: &WsAuthQuery) -> Option<AuthUser> {
    let token = q.token.as_deref()?.trim();
    if token.is_empty() {
        return None;
    }
    crate::routes::auth_from_token(ctx, token)
}

fn make_ticket(ctx: &AppCtx, scope: &str, id: i64) -> Option<String> {
    let exp = (chrono::Utc::now() + chrono::Duration::hours(24)).timestamp() as usize;
    encode(
        &Header::default(),
        &TicketClaims { scope: scope.to_string(), id, exp },
        &EncodingKey::from_secret(ctx.auth_secret.as_bytes()),
    )
    .ok()
}

/// Ticket prüfen; liefert die gebundene ID (Session bzw. Durchsage), wenn Scope passt.
fn check_ticket(ctx: &AppCtx, ticket: &str, scope: &str) -> Option<i64> {
    let data = decode::<TicketClaims>(
        ticket,
        &DecodingKey::from_secret(ctx.auth_secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )
    .ok()?;
    (data.claims.scope == scope).then_some(data.claims.id)
}

/// Darf diese Verbindung das Live-Video sehen? Eingeloggt, oder Video-Ticket für die
/// gerade aktive Session bzw. den laufenden Vor-Start.
pub async fn may_watch_video(ctx: &AppCtx, q: &WsAuthQuery) -> bool {
    if user_from_query(ctx, q).is_some() {
        return true;
    }
    let Some(sid) = q.ticket.as_deref().and_then(|t| check_ticket(ctx, t, "video")) else {
        return false;
    };
    let active = crate::db::sim_active_session_id(&ctx.db).await.ok().flatten();
    let pending = crate::db::settings_get(&ctx.db, "sim.pending_session_id")
        .await
        .ok()
        .flatten()
        .and_then(|s| s.trim().parse::<i64>().ok());
    active == Some(sid) || pending == Some(sid)
}

/// Darf diese Verbindung die laufende Durchsage mithören?
pub fn may_hear_paging(ctx: &AppCtx, q: &WsAuthQuery) -> bool {
    if user_from_query(ctx, q).is_some() {
        return true;
    }
    let current = crate::voice::current_voice_nonce();
    match (q.ticket.as_deref().and_then(|t| check_ticket(ctx, t, "paging")), current) {
        (Some(id), Some(cur)) => id == cur,
        _ => false,
    }
}

/// Legt Befehlen an Viewer/Pager das passende Ticket bei (`show_live` → Video,
/// `paging_start` → Durchsage). Andere Befehle bleiben unverändert.
pub fn attach_ticket(ctx: &AppCtx, cmd: &mut Value) {
    let Some(obj) = cmd.as_object_mut() else { return };
    let ticket = if obj.get("cmd").and_then(|v| v.as_str()) == Some("show_live") {
        obj.get("url")
            .and_then(|u| u.as_str())
            .and_then(|u| u.strip_prefix("/api/webrtc/"))
            .and_then(|sid| sid.trim().parse::<i64>().ok())
            .and_then(|sid| make_ticket(ctx, "video", sid))
    } else if obj.get("action").and_then(|v| v.as_str()) == Some("paging_start") {
        crate::voice::current_voice_nonce().and_then(|n| make_ticket(ctx, "paging", n))
    } else {
        None
    };
    if let Some(t) = ticket {
        obj.insert("ticket".into(), Value::String(t));
    }
}
