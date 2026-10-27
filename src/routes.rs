use crate::app_config;
use crate::db;
use crate::helperscripts::kamera::{self, CameraSimple};
use crate::helperscripts::preview_helper;
use crate::pipeline::RecordOptions as RecordOpts;
use crate::pipeline::SlotGeom as PSGeom;
use crate::sim;
use crate::state::AppCtx;
use async_trait::async_trait;
use axum::headers::{authorization::Bearer, Authorization};
use axum::http::header::CONTENT_TYPE;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::{
    extract::{FromRef, FromRequestParts, Multipart, Path, Query, State},
    response::IntoResponse,
    Json, TypedHeader,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::env;
use std::process::Command;
use tokio::process::Command as TokioCommand;
use tokio::task::spawn_blocking;
use tracing;
// if you have a pipeline reload function:
use crate::db::settings_set;
use crate::main_reload::reload_pipeline; // see main.rs snippet below (rename as needed)
use anyhow::anyhow;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use bytes::Bytes;
use chrono::{Duration, Utc};
use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use password_hash::SaltString;
use rand_core::OsRng;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

const TICKER_KEY: &str = "ui.ticker_messages";
const TRIVIA_KEY: &str = "ui.trivia_images";
const MARKER_NOTES_KEY: &str = "ui.marker_notes.default";
const SETUP_DONE_KEY: &str = "setup.done";
const AUTH_SECRET_KEY: &str = "auth.secret";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TriviaItem {
    file: String,
    title: Option<String>,
}

// ===== Auth Helpers =========================================================
#[derive(Debug, Serialize, Deserialize)]
struct JwtClaims {
    sub: String,
    role: String,
    uid: i64,
    exp: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthUser {
    pub id: i64,
    pub username: String,
    pub role: String,
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    AppCtx: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let State(ctx) = State::<AppCtx>::from_request_parts(parts, state)
            .await
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        let TypedHeader(Authorization(bearer)) =
            TypedHeader::<Authorization<Bearer>>::from_request_parts(parts, state)
                .await
                .map_err(|_| StatusCode::UNAUTHORIZED)?;
        let token = bearer.token();
        let data = decode::<JwtClaims>(
            token,
            &DecodingKey::from_secret(ctx.auth_secret.as_bytes()),
            &Validation::new(Algorithm::HS256),
        )
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
        Ok(AuthUser {
            id: data.claims.uid,
            username: data.claims.sub,
            role: data.claims.role,
        })
    }
}

fn jwt_for_user(ctx: &AppCtx, user: &db::UserRow) -> anyhow::Result<String> {
    let exp = (Utc::now() + Duration::hours(24)).timestamp() as usize;
    let claims = JwtClaims {
        sub: user.username.clone(),
        role: user.role.clone(),
        uid: user.id,
        exp,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(ctx.auth_secret.as_bytes()),
    )?;
    Ok(token)
}

pub(crate) fn auth_from_headers(ctx: &AppCtx, headers: &HeaderMap) -> Option<AuthUser> {
    let auth = headers.get(axum::http::header::AUTHORIZATION)?;
    let auth_str = auth.to_str().ok()?;
    let prefix = "bearer ";
    if !auth_str.to_ascii_lowercase().starts_with(prefix) {
        return None;
    }
    auth_from_token(ctx, auth_str[prefix.len()..].trim())
}

/// Login-Token (JWT) prüfen – für Header und für `?token=` bei WebSockets.
pub(crate) fn auth_from_token(ctx: &AppCtx, token: &str) -> Option<AuthUser> {
    let data = decode::<JwtClaims>(
        token,
        &DecodingKey::from_secret(ctx.auth_secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )
    .ok()?;
    Some(AuthUser {
        id: data.claims.uid,
        username: data.claims.sub,
        role: data.claims.role,
    })
}

fn session_visible_for_user(
    user: Option<&AuthUser>,
    row: &crate::db::SessionRow,
    now_ts: i64,
) -> bool {
    if user.map(|u| u.role == "admin").unwrap_or(false) {
        return true;
    }
    let active = row.record_is_active.unwrap_or(0) == 1;
    if active {
        return true;
    }
    let cutoff = now_ts - 24 * 60 * 60;
    if let Some(stop) = row.stop_time {
        return stop >= cutoff;
    }
    true
}

pub(crate) async fn ensure_session_visible(
    ctx: &AppCtx,
    headers: &HeaderMap,
    session_id: i64,
) -> Result<(), StatusCode> {
    let user = auth_from_headers(ctx, headers);
    let Some(row) = crate::db::session_by_id(&ctx.db, session_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    else {
        return Err(StatusCode::NOT_FOUND);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_else(|_| std::time::Duration::from_secs(0))
        .as_secs() as i64;
    if session_visible_for_user(user.as_ref(), &row, now) {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

pub fn hash_password(pw: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map_err(|e| anyhow!(e.to_string()))?
        .to_string())
}

fn verify_password(hash: &str, pw: &str) -> bool {
    if hash.trim().is_empty() {
        return false;
    }
    if let Ok(parsed) = PasswordHash::new(hash) {
        Argon2::default()
            .verify_password(pw.as_bytes(), &parsed)
            .is_ok()
    } else {
        // Fallback für Legacy-Einträge ohne Argon2-Hash
        hash == pw
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginPayload {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub username: String,
    pub role: String,
    pub user_id: i64,
}

#[derive(Debug, Serialize)]
pub struct PublicUser {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub created_at: i64,
    pub updated_at: Option<i64>,
}

fn map_public_user(u: &db::UserRow) -> PublicUser {
    PublicUser {
        id: u.id,
        username: u.username.clone(),
        role: u.role.clone(),
        created_at: u.created_at,
        updated_at: u.updated_at,
    }
}

fn ensure_admin(user: &AuthUser) -> Result<(), StatusCode> {
    if user.role.to_ascii_lowercase() == "admin" {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

fn truthy(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "y" | "on"
    )
}

async fn setup_is_done(db: &sqlx::SqlitePool) -> bool {
    match db::settings_get(db, SETUP_DONE_KEY).await {
        Ok(Some(v)) => truthy(&v),
        _ => true,
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SetupMarkerType {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SetupCamera {
    pub name: String,
    pub url: String,
    pub mjpeg_url: Option<String>,
    pub fov_deg: Option<f64>,
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SetupSettings {
    pub record_enabled: bool,
    pub record_format: String,
    pub record_segment_secs: u32,
    pub record_max_files: i32,
    pub record_prefix: String,
    pub record_dir: String,
    pub audio_device: String,
}

#[derive(Debug, Serialize)]
pub struct SetupDefaults {
    pub admin_username: String,
    pub auth_secret: String,
    pub marker_types: Vec<SetupMarkerType>,
    pub marker_notes: Vec<String>,
    pub cameras: Vec<SetupCamera>,
    pub settings: SetupSettings,
}

#[derive(Debug, Serialize)]
pub struct SetupStatusResponse {
    pub setup_done: bool,
    pub defaults: Option<SetupDefaults>,
}

#[derive(Debug, Deserialize)]
pub struct SetupPayload {
    pub admin_username: String,
    pub admin_password: String,
    pub auth_secret: String,
    pub marker_types: Option<Vec<SetupMarkerType>>,
    pub marker_notes: Option<Vec<String>>,
    pub cameras: Option<Vec<SetupCamera>>,
    pub settings: Option<SetupSettings>,
}

fn default_marker_types() -> Vec<SetupMarkerType> {
    vec![
        SetupMarkerType {
            name: "Beginn".to_string(),
            color: "#2ecc71".to_string(),
        },
        SetupMarkerType {
            name: "Einsatz".to_string(),
            color: "#f1c40f".to_string(),
        },
        SetupMarkerType {
            name: "Hinweis".to_string(),
            color: "#3498db".to_string(),
        },
        SetupMarkerType {
            name: "Problem".to_string(),
            color: "#e74c3c".to_string(),
        },
    ]
}

async fn build_setup_defaults(db: &sqlx::SqlitePool) -> SetupDefaults {
    let marker_types_existing = db::marker_types_list(db, None).await.unwrap_or_default();
    let marker_types = if marker_types_existing.is_empty() {
        default_marker_types()
    } else {
        marker_types_existing
            .into_iter()
            .map(|m| SetupMarkerType {
                name: m.name,
                color: m.color,
            })
            .collect()
    };

    let cameras_existing = db::cameras_list(db).await.unwrap_or_default();
    let cameras = cameras_existing
        .into_iter()
        .map(|c| SetupCamera {
            name: c.name,
            url: c.url,
            mjpeg_url: c.mjpeg_url,
            fov_deg: c.fov_deg,
            enabled: c.enabled != 0,
        })
        .collect();

    let marker_notes = load_marker_notes(db, None).await;
    let rec_dir_default = crate::db::app_dirs().rec_dir;
    let record_dir = app_config::record_dir(db, &rec_dir_default.to_string_lossy()).await;
    let record_prefix = app_config::record_prefix(db, "rec").await;
    let record_enabled = app_config::record_enabled(db).await;
    let record_format = app_config::record_container(db).await;
    let record_segment_secs = app_config::record_segment_secs(db, 300).await;
    let record_max_files = app_config::record_max_files(db).await;
    let audio_device = app_config::audio_device(db)
        .await
        .unwrap_or_else(|| "default".to_string());

    let auth_secret = db::settings_get(db, AUTH_SECRET_KEY)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    SetupDefaults {
        admin_username: "admin".to_string(),
        auth_secret,
        marker_types,
        marker_notes,
        cameras,
        settings: SetupSettings {
            record_enabled,
            record_format,
            record_segment_secs,
            record_max_files,
            record_prefix,
            record_dir,
            audio_device,
        },
    }
}

pub async fn setup_status(State(ctx): State<AppCtx>) -> Result<Json<SetupStatusResponse>, StatusCode> {
    if setup_is_done(&ctx.db).await {
        return Ok(Json(SetupStatusResponse {
            setup_done: true,
            defaults: None,
        }));
    }
    let defaults = build_setup_defaults(&ctx.db).await;
    Ok(Json(SetupStatusResponse {
        setup_done: false,
        defaults: Some(defaults),
    }))
}

pub async fn setup_complete(
    State(ctx): State<AppCtx>,
    Json(body): Json<SetupPayload>,
) -> Result<Json<JsonValue>, StatusCode> {
    if setup_is_done(&ctx.db).await {
        return Err(StatusCode::CONFLICT);
    }

    let admin_username = body.admin_username.trim().to_string();
    let admin_password = body.admin_password.trim().to_string();
    let auth_secret = body.auth_secret.trim().to_string();
    if admin_username.is_empty() || admin_password.is_empty() || auth_secret.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let settings = body.settings.ok_or(StatusCode::BAD_REQUEST)?;
    let record_format = settings.record_format.to_ascii_lowercase();
    if record_format != "mkv" && record_format != "mp4" && record_format != "mov" {
        return Err(StatusCode::BAD_REQUEST);
    }
    if settings.record_segment_secs == 0 {
        return Err(StatusCode::BAD_REQUEST);
    }

    let admin_hash =
        hash_password(&admin_password).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    db::ensure_admin_user(&ctx.db, &admin_username, &admin_hash)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let _ = settings_set(&ctx.db, AUTH_SECRET_KEY, &auth_secret).await;
    let _ = settings_set(
        &ctx.db,
        "record.enabled",
        if settings.record_enabled { "true" } else { "false" },
    )
    .await;
    let _ = settings_set(&ctx.db, "record.format", &record_format).await;
    let _ = settings_set(
        &ctx.db,
        "record.segment_secs",
        &settings.record_segment_secs.to_string(),
    )
    .await;
    let _ = settings_set(
        &ctx.db,
        "record.max_files",
        &settings.record_max_files.to_string(),
    )
    .await;
    let _ = settings_set(&ctx.db, "record.prefix", settings.record_prefix.trim()).await;
    let _ = settings_set(&ctx.db, "record.dir", settings.record_dir.trim()).await;
    let _ = settings_set(&ctx.db, "audio.device", settings.audio_device.trim()).await;

    if let Some(notes) = body.marker_notes {
        let _ = save_marker_notes(&ctx.db, None, &notes).await;
    }

    let mut marker_types = body.marker_types.unwrap_or_default();
    if marker_types.is_empty() {
        marker_types = default_marker_types();
    }
    let existing_marker_types = db::marker_types_list(&ctx.db, None).await.unwrap_or_default();
    let mut marker_by_name: HashMap<String, db::MarkerType> = HashMap::new();
    for t in existing_marker_types {
        marker_by_name.insert(t.name.to_ascii_lowercase(), t);
    }
    for mt in marker_types {
        let name = mt.name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        let color = if mt.color.trim().is_empty() {
            "#ff0000".to_string()
        } else {
            mt.color.trim().to_string()
        };
        if let Some(existing) = marker_by_name.get(&name.to_ascii_lowercase()) {
            let sort_index = existing
                .sort_index
                .or_else(|| db::marker_type_default_sort_index(&name));
            let _ = db::marker_types_update(&ctx.db, existing.id, &name, &color, sort_index).await;
        } else {
            let sort_index = if let Some(v) = db::marker_type_default_sort_index(&name) {
                Some(v)
            } else {
                db::marker_types_next_sort_index(&ctx.db, None).await.ok()
            };
            let _ = db::marker_types_add(&ctx.db, &name, &color, None, sort_index).await;
        }
    }

    let cameras = body.cameras.unwrap_or_default();
    let existing_cameras = db::cameras_list(&ctx.db).await.unwrap_or_default();
    let mut cameras_by_name: HashMap<String, db::Camera> = HashMap::new();
    for cam in existing_cameras {
        cameras_by_name.insert(cam.name.to_ascii_lowercase(), cam);
    }
    for cam in cameras {
        let name = cam.name.trim().to_string();
        let url = cam.url.trim().to_string();
        let mjpeg_url = cam
            .mjpeg_url
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if name.is_empty() || url.is_empty() {
            continue;
        }
        if let Some(existing) = cameras_by_name.get(&name.to_ascii_lowercase()) {
            // PTZ/Brand-Felder bleiben unangetastet (Setup-Wizard kennt sie nicht).
            let _ = db::cameras_update(
                &ctx.db,
                existing.id,
                &name,
                &url,
                mjpeg_url.as_deref(),
                cam.fov_deg,
                cam.enabled,
                &existing.brand,
                existing.ptz_capable != 0,
                existing.vapix_username.as_deref(),
                existing.vapix_password.as_deref(),
                existing.vapix_port,
            )
            .await;
        } else {
            let id = db::cameras_add(
                &ctx.db,
                &name,
                &url,
                mjpeg_url.as_deref(),
                cam.fov_deg,
                db::CameraPtzFields::default(),
            )
            .await
            .unwrap_or(0);
            if id > 0 && !cam.enabled {
                let _ = db::cameras_update(
                    &ctx.db,
                    id,
                    &name,
                    &url,
                    mjpeg_url.as_deref(),
                    cam.fov_deg,
                    false,
                    "generic",
                    false,
                    None,
                    None,
                    None,
                )
                .await;
            }
        }
    }

    let _ = settings_set(&ctx.db, SETUP_DONE_KEY, "true").await;

    Ok(Json(json!({ "ok": true, "requires_restart": true })))
}

#[derive(Debug, Serialize)]
pub struct AdminSettingsDto {
    pub output_rtmp: Option<String>,
    pub audio_device: Option<String>,
    pub record_enabled: bool,
    pub record_dir: Option<String>,
    pub record_prefix: Option<String>,
    pub record_format: Option<String>,
    pub record_segment_secs: Option<i64>,
    pub record_max_files: Option<i32>,
    pub log_retention_days: Option<i64>,
    pub log_mail_to: Option<String>,
    pub env: Vec<EnvEntry>,
}

#[derive(Debug, Deserialize)]
pub struct AdminSettingsPayload {
    pub output_rtmp: Option<String>,
    pub audio_device: Option<String>,
    pub record_enabled: Option<bool>,
    pub record_dir: Option<String>,
    pub record_prefix: Option<String>,
    pub record_format: Option<String>,
    pub record_segment_secs: Option<i64>,
    pub record_max_files: Option<i32>,
    pub log_retention_days: Option<i64>,
    pub log_mail_to: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EnvEntry {
    pub key: String,
    pub value: String,
}

pub async fn auth_login(
    State(ctx): State<AppCtx>,
    Json(body): Json<LoginPayload>,
) -> Result<Json<LoginResponse>, (StatusCode, Json<JsonValue>)> {
    if body.username.trim().is_empty() || body.password.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "username/password fehlt"})),
        ));
    }
    let Some(user) = db::user_by_username(&ctx.db, &body.username)
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "db"})),
            )
        })?
    else {
        return Err((StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid"}))));
    };
    let hash = match user.password_hash.as_deref() {
        Some(h) if !h.is_empty() => h,
        _ => return Err((StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid"})))),
    };
    if !verify_password(hash, &body.password) {
        return Err((StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid"}))));
    }
    let token = jwt_for_user(&ctx, &user).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "token"})),
        )
    })?;
    Ok(Json(LoginResponse {
        access_token: token,
        username: user.username,
        role: user.role,
        user_id: user.id,
    }))
}

#[derive(Debug, Deserialize)]
pub struct RegisterPayload {
    pub token: String,
    pub username: String,
    pub password: String,
}

pub async fn auth_register(
    State(ctx): State<AppCtx>,
    Json(body): Json<RegisterPayload>,
) -> Result<Json<LoginResponse>, (StatusCode, Json<JsonValue>)> {
    let token = body.token.trim().to_string();
    if token.is_empty() || body.username.trim().is_empty() || body.password.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "token/username/password fehlt"})),
        ));
    }

    let Some(inv) = db::invite_token_get(&ctx.db, &token)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "db"}))))?
    else {
        return Err((StatusCode::FORBIDDEN, Json(json!({"error": "token_invalid"}))));
    };
    if inv.used_at.is_some() {
        return Err((StatusCode::FORBIDDEN, Json(json!({"error": "token_used"}))));
    }

    let hash = hash_password(&body.password)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "hash"}))))?;
    let user = db::user_insert(&ctx.db, &body.username, &hash, "user")
        .await
        .map_err(|_| (StatusCode::CONFLICT, Json(json!({"error": "username_taken"}))))?;

    let marked = db::invite_token_mark_used(&ctx.db, &token, user.id)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "db"}))))?;
    if !marked {
        return Err((StatusCode::FORBIDDEN, Json(json!({"error": "token_used"}))));
    }

    let jwt = jwt_for_user(&ctx, &user)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "token"}))))?;

    Ok(Json(LoginResponse {
        access_token: jwt,
        username: user.username,
        role: user.role,
        user_id: user.id,
    }))
}

pub async fn auth_me(user: AuthUser) -> Json<LoginResponse> {
    Json(LoginResponse {
        access_token: String::new(),
        username: user.username,
        role: user.role,
        user_id: user.id,
    })
}

pub async fn admin_settings_get(
    State(ctx): State<AppCtx>,
    user: AuthUser,
) -> Result<Json<AdminSettingsDto>, StatusCode> {
    ensure_admin(&user)?;
    let output_rtmp = crate::db::settings_get(&ctx.db, "output.rtmp")
        .await
        .ok()
        .flatten();
    let audio_device = crate::db::settings_get(&ctx.db, "audio.device")
        .await
        .ok()
        .flatten();
    let record_enabled = crate::db::settings_get(&ctx.db, "record.enabled")
        .await
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false);
    let record_dir = crate::db::settings_get(&ctx.db, "record.dir")
        .await
        .ok()
        .flatten();
    let record_prefix = crate::db::settings_get(&ctx.db, "record.prefix")
        .await
        .ok()
        .flatten();
    let record_format = crate::db::settings_get(&ctx.db, "record.format")
        .await
        .ok()
        .flatten();
    let record_segment_secs = crate::db::settings_get(&ctx.db, "record.segment_secs")
        .await
        .ok()
        .flatten()
        .and_then(|s| s.parse::<i64>().ok());
    let record_max_files = crate::db::settings_get(&ctx.db, "record.max_files")
        .await
        .ok()
        .flatten()
        .and_then(|s| s.parse::<i32>().ok());
    let log_retention_days = crate::db::settings_get(&ctx.db, "logs.retention_days")
        .await
        .ok()
        .flatten()
        .and_then(|s| s.parse::<i64>().ok());
    let log_mail_to = crate::db::settings_get(&ctx.db, "logs.mail_to")
        .await
        .ok()
        .flatten();

    // Read-only ENV snapshot (non-sensitive)
    let mut env_entries: Vec<EnvEntry> = Vec::new();
    for k in [
        "ORS_BIND",
        "ORS_DATA_DIR",
        "ORS_DB_PATH",
        "ORS_LOG_DIR",
        "ORS_LOG_FILE",
        "ORS_JOURNAL_UNIT",
        "ORS_SYSTEMD_UNIT",
        "ORS_RTSP_PROTOCOLS",
        "ORS_RTSP_INSECURE",
    ] {
        if let Ok(v) = std::env::var(k) {
            env_entries.push(EnvEntry {
                key: k.to_string(),
                value: v,
            });
        }
    }

    Ok(Json(AdminSettingsDto {
        output_rtmp,
        audio_device,
        record_enabled,
        record_dir,
        record_prefix,
        record_format,
        record_segment_secs,
        record_max_files,
        log_retention_days,
        log_mail_to,
        env: env_entries,
    }))
}

pub async fn admin_settings_set(
    State(ctx): State<AppCtx>,
    user: AuthUser,
    Json(body): Json<AdminSettingsPayload>,
) -> Result<Json<JsonValue>, StatusCode> {
    ensure_admin(&user)?;

    if let Some(v) = body.output_rtmp {
        let _ = settings_set(&ctx.db, "output.rtmp", v.trim()).await;
    }
    if let Some(v) = body.audio_device {
        let _ = settings_set(&ctx.db, "audio.device", v.trim()).await;
    }
    if let Some(v) = body.record_enabled {
        let _ = settings_set(&ctx.db, "record.enabled", if v { "true" } else { "false" }).await;
    }
    if let Some(v) = body.record_dir {
        let _ = settings_set(&ctx.db, "record.dir", v.trim()).await;
    }
    if let Some(v) = body.record_prefix {
        let _ = settings_set(&ctx.db, "record.prefix", v.trim()).await;
    }
    if let Some(v) = body.record_format {
        let fmt = v.trim().to_lowercase();
        let _ = settings_set(&ctx.db, "record.format", &fmt).await;
    }
    if let Some(v) = body.record_segment_secs {
        let _ = settings_set(&ctx.db, "record.segment_secs", &v.to_string()).await;
    }
    if let Some(v) = body.record_max_files {
        let _ = settings_set(&ctx.db, "record.max_files", &v.to_string()).await;
    }
    if let Some(v) = body.log_retention_days {
        let clamped = v.clamp(0, 3650);
        let _ = settings_set(&ctx.db, "logs.retention_days", &clamped.to_string()).await;
    }
    if let Some(v) = body.log_mail_to {
        let _ = settings_set(&ctx.db, "logs.mail_to", v.trim()).await;
    }

    // Pipeline neu aufbauen, damit Änderungen wirken
    reload_pipeline().await;

    Ok(Json(json!({ "ok": true })))
}

fn updates_dir() -> std::path::PathBuf {
    let base = crate::db::app_dirs().base_dir;
    base.join("updates")
}

fn trivia_dir() -> std::path::PathBuf {
    let base = crate::db::app_dirs().base_dir;
    base.join("trivia")
}

async fn load_ticker_messages(db: &sqlx::SqlitePool) -> Vec<String> {
    crate::db::settings_get(db, TICKER_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_else(|| {
            vec![
                "Jeder macht, kann und darf Fehler machen - keine Fehler = kein Training"
                    .to_string(),
                "Im Zentrum: Teamarbeit, Entscheidungsfindung".to_string(),
            ]
        })
}

async fn save_ticker_messages(db: &sqlx::SqlitePool, msgs: &[String]) -> Result<(), StatusCode> {
    let filtered: Vec<String> = msgs
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let json = serde_json::to_string(&filtered).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    crate::db::settings_set(db, TICKER_KEY, &json)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn load_trivia_items(db: &sqlx::SqlitePool) -> Vec<TriviaItem> {
    crate::db::settings_get(db, TRIVIA_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<Vec<TriviaItem>>(&s).ok())
        .unwrap_or_default()
}

async fn save_trivia_items(db: &sqlx::SqlitePool, list: &[TriviaItem]) -> Result<(), StatusCode> {
    let json = serde_json::to_string(list).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    crate::db::settings_set(db, TRIVIA_KEY, &json)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn marker_notes_key(user_id: Option<i64>) -> String {
    match user_id {
        Some(uid) => format!("ui.marker_notes.user.{uid}"),
        None => MARKER_NOTES_KEY.to_string(),
    }
}

async fn load_marker_notes(db: &sqlx::SqlitePool, user_id: Option<i64>) -> Vec<String> {
    let default_notes = vec![
        "Patient stabil".to_string(),
        "Transport bereit".to_string(),
        "Team unterwegs".to_string(),
        "Rettungsweg frei".to_string(),
    ];
    let key = marker_notes_key(user_id);
    let notes = crate::db::settings_get(db, &key)
        .await
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    if !notes.is_empty() || user_id.is_none() {
        return if notes.is_empty() { default_notes } else { notes };
    }
    crate::db::settings_get(db, MARKER_NOTES_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or(default_notes)
}

async fn save_marker_notes(
    db: &sqlx::SqlitePool,
    user_id: Option<i64>,
    notes: &[String],
) -> Result<(), StatusCode> {
    let filtered: Vec<String> = notes
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let json = serde_json::to_string(&filtered).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let key = marker_notes_key(user_id);
    crate::db::settings_set(db, &key, &json)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn admin_update_list(user: AuthUser) -> Result<Json<JsonValue>, StatusCode> {
    ensure_admin(&user)?;
    let dir = updates_dir();
    let mut list = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            if let Ok(ft) = entry.file_type().await {
                if ft.is_file() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let path = entry.path();
                    if name.ends_with(".deb") {
                        let meta = tokio::fs::metadata(&path).await.ok();
                        list.push(json!({
                            "name": name,
                            "path": path.to_string_lossy(),
                            "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
                            "modified": meta
                                .and_then(|m| m.modified().ok())
                                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).timestamp()),
                        }));
                    }
                }
            }
        }
    }
    Ok(Json(json!({ "files": list })))
}

pub async fn admin_update_upload(
    user: AuthUser,
    mut multipart: Multipart,
) -> Result<Json<JsonValue>, StatusCode> {
    ensure_admin(&user)?;
    let dir = updates_dir();
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        error!(?e, "cannot create updates dir");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    while let Ok(Some(field)) = multipart.next_field().await {
        if let Some(name) = field.name() {
            if name != "file" {
                continue;
            }
        }
        let filename = field
            .file_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "update.deb".into());
        if !filename.ends_with(".deb") {
            return Err(StatusCode::BAD_REQUEST);
        }
        let data = field.bytes().await.map_err(|_| StatusCode::BAD_REQUEST)?;
        let target = dir.join(&filename);
        if let Err(e) = tokio::fs::write(&target, &data).await {
            error!(?e, "write upload failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        return Ok(Json(json!({
            "ok": true,
            "path": target.to_string_lossy(),
            "name": filename,
            "size": data.len()
        })));
    }

    Err(StatusCode::BAD_REQUEST)
}

#[derive(Debug, Deserialize)]
pub struct AdminInstallPayload {
    pub path: String,
}

pub async fn admin_update_install(
    user: AuthUser,
    Json(p): Json<AdminInstallPayload>,
) -> Result<Json<JsonValue>, StatusCode> {
    ensure_admin(&user)?;
    info!(user_id = user.id, path = %p.path, "update install requested");
    let dir = updates_dir();
    let requested = std::path::PathBuf::from(&p.path);
    let canonical = match requested.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            warn!(user_id = user.id, path = %p.path, ?e, "update install: canonicalize failed");
            return Err(StatusCode::BAD_REQUEST);
        }
    };
    if !canonical.starts_with(&dir) {
        warn!(
            user_id = user.id,
            requested = %p.path,
            canonical = %canonical.display(),
            "update install: path outside updates dir"
        );
        return Err(StatusCode::FORBIDDEN);
    }
    if canonical.extension().and_then(|s| s.to_str()) != Some("deb") {
        warn!(
            user_id = user.id,
            canonical = %canonical.display(),
            "update install: invalid extension"
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    info!(
        user_id = user.id,
        canonical = %canonical.display(),
        "update install: running dpkg -i"
    );
    let output = TokioCommand::new("dpkg")
        .arg("-i")
        .arg(&canonical)
        .output()
        .await
        .map_err(|e| {
            error!(
                user_id = user.id,
                canonical = %canonical.display(),
                ?e,
                "update install: dpkg execution failed"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let ok = output.status.success();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    info!(
        user_id = user.id,
        ok,
        status = output.status.code(),
        stdout_len = stdout.len(),
        stderr_len = stderr.len(),
        "update install: dpkg finished"
    );
    if !ok {
        warn!(
            user_id = user.id,
            status = output.status.code(),
            "update install: dpkg returned non-zero"
        );
    }

    if ok {
        reload_pipeline().await;
    }

    Ok(Json(json!({
        "ok": ok,
        "status": output.status.code(),
        "stdout": stdout,
        "stderr": stderr
    })))
}

#[derive(Debug, Deserialize)]
pub struct UserCreatePayload {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub role: Option<String>,
}

pub async fn users_list_handler(
    State(ctx): State<AppCtx>,
    user: AuthUser,
) -> Result<Json<JsonValue>, StatusCode> {
    ensure_admin(&user)?;
    let users = db::users_list(&ctx.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let list: Vec<PublicUser> = users
        .iter()
        .filter(|u| u.role != db::PARTICIPANT_ROLE)
        .map(map_public_user)
        .collect();
    Ok(Json(json!({ "users": list })))
}

pub async fn users_create_handler(
    State(ctx): State<AppCtx>,
    user: AuthUser,
    Json(body): Json<UserCreatePayload>,
) -> Result<Json<JsonValue>, StatusCode> {
    ensure_admin(&user)?;
    if body.username.trim().is_empty() || body.password.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let role = body.role.clone().unwrap_or_else(|| "user".into());
    let hash = hash_password(&body.password).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match db::user_insert(&ctx.db, &body.username, &hash, &role).await {
        Ok(u) => Ok(Json(json!({ "user": map_public_user(&u) }))),
        Err(e) => {
            error!(?e, "users_create failed");
            Err(StatusCode::CONFLICT)
        }
    }
}

pub async fn users_delete_handler(
    State(ctx): State<AppCtx>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<JsonValue>, StatusCode> {
    ensure_admin(&user)?;
    if id == user.id {
        return Err(StatusCode::BAD_REQUEST);
    }
    db::user_delete(&ctx.db, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn user_stats_handler(
    State(ctx): State<AppCtx>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<JsonValue>, StatusCode> {
    if user.id != id && user.role != "admin" {
        return Err(StatusCode::FORBIDDEN);
    }
    let stats = crate::achievement::get_user_stats(&ctx.db, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "user_id": id, "stats": stats })))
}

pub async fn user_achievements_handler(
    State(ctx): State<AppCtx>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<JsonValue>, StatusCode> {
    if user.id != id && user.role != "admin" {
        return Err(StatusCode::FORBIDDEN);
    }
    let list = crate::achievement::list_user_achievements(&ctx.db, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "user_id": id, "achievements": list })))
}

#[derive(Deserialize)]
pub struct QrCodeQuery {
    pub text: Option<String>,
}

pub async fn qr_code_svg(
    Query(q): Query<QrCodeQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let text = q.text.unwrap_or_default();
    let text = text.trim();
    if text.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let code = qrcode::QrCode::with_error_correction_level(
        text.as_bytes(),
        qrcode::EcLevel::M,
    )
    .map_err(|_| StatusCode::BAD_REQUEST)?;
    let image = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .build();
    Ok(([(CONTENT_TYPE, "image/svg+xml; charset=utf-8")], image))
}

#[derive(Debug, Serialize)]
pub struct InviteTokenDto {
    pub token: String,
}

pub async fn admin_invite_token_create(
    State(ctx): State<AppCtx>,
    user: AuthUser,
) -> Result<Json<InviteTokenDto>, StatusCode> {
    ensure_admin(&user)?;
    let token = db::invite_token_create(&ctx.db, Some(user.id))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(InviteTokenDto { token }))
}

#[derive(Deserialize)]
pub struct AdminUserStatsQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

pub async fn admin_user_stats_handler(
    State(ctx): State<AppCtx>,
    user: AuthUser,
    Query(q): Query<AdminUserStatsQuery>,
) -> Result<Json<JsonValue>, StatusCode> {
    ensure_admin(&user)?;
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(25).clamp(5, 200);
    let offset = (page - 1) * page_size;
    let (rows, total) = crate::achievement::list_all_user_stats_paginated(&ctx.db, page_size, offset)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(user_id, username, role, stats)| {
            json!({
                "user_id": user_id,
                "username": username,
                "role": role,
                "stats": stats
            })
        })
        .collect();
    let total_pages = if total == 0 { 1 } else { (total + page_size - 1) / page_size };
    Ok(Json(json!({
        "users": out,
        "page": page,
        "page_size": page_size,
        "total": total,
        "total_pages": total_pages
    })))
}

use gstreamer::prelude::ElementExtManual;
use std::path::Path as FsPath;
// sysinfo optional: remove if not available
use tokio::fs;

// UI-Broadcast helper
pub async fn broadcast_ui(ctx: &AppCtx, msg: serde_json::Value) {
    let msg_str = msg.to_string();
    let mut chans = ctx.ui_broadcast.lock().await;
    chans.retain(|tx| tx.send(msg_str.clone()).is_ok());
}

// Webhook-Helfer: sendet Events, wenn WEBHOOK_URL gesetzt ist
pub(crate) async fn trigger_webhook(event: &str, payload: JsonValue) {
    let url = match std::env::var("WEBHOOK_URL") {
        Ok(u) if !u.trim().is_empty() => u,
        _ => return,
    };

    let body = json!({
        "event": event,
        "ts": chrono::Utc::now().to_rfc3339(),
        "payload": payload
    });

    tokio::spawn(async move {
        let client = reqwest::Client::new();
        if let Err(e) = client.post(&url).json(&body).send().await {
            tracing::warn!(?e, %url, "webhook failed");
        }
    });
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetLayoutPayload {
    // freie Bezeichnung (optional)
    pub name: Option<String>,
    // Geometrie pro Platz: index == Platznummer (0..N-1)
    pub geoms: Vec<SlotGeom>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SlotGeom {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub z: i32, // wird als u32 in zorder gecastet
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetSourcePayload {
    pub place: usize, // welcher Platz (0..N-1)
    pub source: SourceSpec,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
#[allow(dead_code)] // Felder kommen aus dem Request, werden derzeit nur validiert
pub enum SourceSpec {
    Test {
        pattern: Option<i32>,
    }, // videotestsrc
    Rtsp {
        url: String,
        latency_ms: Option<u32>,
    }, // Beispiel
       // weitere Typen nach Bedarf…
}

pub async fn set_layout(
    State(ctx): State<AppCtx>,
    Json(p): Json<SetLayoutPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let geoms_for_pipeline: Vec<PSGeom> = p
        .geoms
        .iter()
        .map(|g| PSGeom {
            x: g.x,
            y: g.y,
            w: g.w,
            h: g.h,
            z: g.z,
        })
        .collect();

    // 2) Live-Layout anwenden
    let apply_err = {
        let mut pm = ctx.pipeline.lock().await;
        match pm.apply_layout(&geoms_for_pipeline) {
            Ok(()) => {
                pm.set_cached_layout(geoms_for_pipeline.clone());
                None
            }
            Err(e) => Some(e),
        }
    };

    if let Some(e) = apply_err {
        eprintln!("apply_layout live failed: {e:?}");

        // ---- Fallback: Rebuild ----
        let st = ctx.status.lock().await.clone();
        let out_rtmp = crate::db::settings_get(&ctx.db, "output.rtmp")
            .await
            .ok()
            .flatten();
        let audio_dev = crate::db::settings_get(&ctx.db, "audio.device")
            .await
            .ok()
            .flatten();

        let rec_enabled = crate::db::settings_get(&ctx.db, "record.enabled")
            .await
            .ok()
            .flatten()
            .map(|v| v == "true")
            .unwrap_or(false);

        let rec_opts = if rec_enabled {
            let dirs = crate::db::app_dirs();
            let dir = crate::db::settings_get(&ctx.db, "record.dir")
                .await
                .ok()
                .flatten()
                .unwrap_or_else(|| dirs.rec_dir.to_string_lossy().to_string());
            let basename = crate::db::settings_get(&ctx.db, "record.prefix")
                .await
                .ok()
                .flatten()
                .unwrap_or_else(|| "rec".into());
            let split_ms = crate::db::settings_get(&ctx.db, "record.segment_secs")
                .await
                .ok()
                .flatten()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(300)
                * 1000;
            let container = crate::db::settings_get(&ctx.db, "record.format")
                .await
                .ok()
                .flatten()
                .map(|s| s.to_lowercase())
                .unwrap_or_else(|| "mp4".into());
            let max_files_u32 = crate::db::settings_get(&ctx.db, "record.max_files")
                .await
                .ok()
                .flatten()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);

            Some(crate::pipeline::RecordOptions {
                enabled: true,
                directory: dir,
                basename,
                container,
                split_ms,
                max_files: i32::try_from(max_files_u32).unwrap_or(0), // i32 erwartet
                audio_dev: audio_dev.clone(),
            })
        } else {
            None
        };

        {
            let mut pm = ctx.pipeline.lock().await;
            // gewünschtes Layout cachen, damit es nach Rebuild greift
            pm.set_cached_layout(geoms_for_pipeline.clone());

            pm.rebuild(
                &st,
                out_rtmp.as_deref(),
                audio_dev.as_deref(),
                rec_opts,
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
    }

    Ok(Json(serde_json::json!({ "ok": true, "name": p.name })))
}

pub async fn set_source(
    State(ctx): State<AppCtx>,
    user: AuthUser,
    Json(p): Json<SetSourcePayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    ensure_admin(&user)?;
    let _ = p.source;
    let mut pm = ctx.pipeline.lock().await;
    pm.set_source(p.place as i32, None::<String>)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// Beispiel: vorhandene Layout-Presets (kannst du später aus DB lesen)
#[derive(Debug, Clone, Serialize)]
pub struct LayoutPreset {
    pub name: String,
    pub geoms: Vec<SlotGeom>,
}

pub async fn layouts_list_handler(State(_ctx): State<AppCtx>) -> Json<Vec<LayoutPreset>> {
    // Minimal zwei Presets
    Json(vec![
        LayoutPreset {
            name: "single_full".into(),
            geoms: vec![SlotGeom {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
                z: 0,
            }],
        },
        LayoutPreset {
            name: "2x2".into(),
            geoms: vec![
                SlotGeom {
                    x: 0,
                    y: 0,
                    w: 960,
                    h: 540,
                    z: 0,
                },
                SlotGeom {
                    x: 960,
                    y: 0,
                    w: 960,
                    h: 540,
                    z: 1,
                },
                SlotGeom {
                    x: 0,
                    y: 540,
                    w: 960,
                    h: 540,
                    z: 2,
                },
                SlotGeom {
                    x: 960,
                    y: 540,
                    w: 960,
                    h: 540,
                    z: 3,
                },
            ],
        },
    ])
}

pub async fn rebuild_handler(State(ctx): State<AppCtx>) -> (StatusCode, Json<serde_json::Value>) {
    info!("/api/rebuild called");

    // App-Status snapshot
    let st = ctx.status.lock().await.clone();

    // --- Outputs/Devices ---
    let out_rtmp = crate::db::settings_get(&ctx.db, "output.rtmp")
        .await
        .ok()
        .flatten();

    let audio_dev = crate::db::settings_get(&ctx.db, "audio.device")
        .await
        .ok()
        .flatten();


    // --- Recording options ---
    let rec_enabled = crate::db::settings_get(&ctx.db, "record.enabled")
        .await
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false);

    let rec_opts: Option<RecordOpts> = if rec_enabled {
        // Zielverzeichnis
        let rec_dir = crate::db::settings_get(&ctx.db, "record.dir")
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                format!(
                    "{}/.local/share/openresquesim/recordings",
                    std::env::var("HOME").unwrap_or_else(|_| ".".into())
                )
            });

        // Prefix/Basename
        let rec_base = crate::db::settings_get(&ctx.db, "record.prefix")
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "rec".into());

        // Segmentlänge: Sekunden -> Millisekunden
        let rec_segment_ms: i64 = crate::db::settings_get(&ctx.db, "record.segment_secs")
            .await
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u32>().ok())
            .map(|secs| secs as i64 * 1000)
            .unwrap_or(300_000);

        // Rotationslimit (0 = unlimitiert)
        let rec_max_files: i32 = crate::db::settings_get(&ctx.db, "record.max_files")
            .await
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);

        // Container laut App-Config ("mp4" | "mkv" | "mov")
        let rec_container = app_config::record_container(&ctx.db).await;

        // Verzeichnis anlegen (falls fehlt)
        if let Err(e) = std::fs::create_dir_all(&rec_dir) {
            error!(dir=%rec_dir, ?e, "failed to create record dir");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": format!("create record dir: {e}") })),
            );
        }

        info!(
            enabled = true,
            dir = %rec_dir,
            base = %rec_base,
            seg_ms = rec_segment_ms,
            max_files = rec_max_files,
            container = %rec_container,
            "⏺️ record options"
        );

        Some(RecordOpts {
            enabled: true,
            directory: rec_dir,
            basename: rec_base,
            container: rec_container,
            split_ms: rec_segment_ms,
            max_files: rec_max_files,
            audio_dev: audio_dev.clone(),
        })
    } else {
        info!(enabled = false, "⏺️ record disabled");
        None
    };

    // --- Logging zur Laufzeitkonfiguration ---
    if let Some(ref s) = out_rtmp {
        info!(rtmp = %crate::pipeline::mask_stream_key(s), "RTMP output configured");
    }
    if let Some(ref d) = audio_dev {
        info!(audio_dev = %d, "Audio device configured");
    }

    // --- Rebuild ausführen ---
    let mut pm = ctx.pipeline.lock().await;
    info!("PipelineManager::rebuild() enter");
    let res = pm.rebuild(
        &st,
        out_rtmp.as_deref(),
        audio_dev.as_deref(),
        rec_opts,
    );

    match res {
        Ok(()) => {
            info!("PipelineManager::rebuild() OK (pipeline should be PLAYING)");
            (StatusCode::OK, Json(json!({ "ok": true })))
        }
        Err(e) => {
            error!(?e, "PipelineManager::rebuild() ERROR");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": e.to_string() })),
            )
        }
    }
}

// -- Cameras (DB) ---------------------------------------------------

pub async fn cameras_list_handler(
    State(ctx): State<AppCtx>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let rows = crate::db::cameras_list(&ctx.db).await.map_err(|e| {
        error!(?e, "list cameras");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(json!({ "cameras": rows })))
}

/// GET /api/camera/brands
pub async fn camera_brands() -> axum::Json<serde_json::Value> {
    let list = kamera::supported_vendors();
    axum::Json(serde_json::json!({ "brands": list }))
}

/// POST /api/camera/add_simple
/// Payload: CameraSimple + { index: usize }
#[derive(serde::Deserialize)]
pub struct CameraAddSimplePayload {
    pub index: usize,
    #[serde(flatten)]
    pub cam: CameraSimple,
}

pub async fn camera_add_simple(
    axum::extract::State(ctx): axum::extract::State<crate::state::AppCtx>,
    user: AuthUser,
    axum::Json(p): axum::Json<CameraAddSimplePayload>,
) -> Result<axum::Json<serde_json::Value>, StatusCode> {
    ensure_admin(&user)?;
    // URLs bauen
    let built = kamera::build_uris(&p.cam);

    // <- WICHTIG: vorher klonen, sonst sind die Options später "verbraucht"
    let rtsp = built.rtsp.clone();
    let rtmp = built.rtmp.clone();

    // Wir bevorzugen RTSP, sonst RTMP, sonst leer
    let uri = rtsp.clone().or(rtmp.clone()).unwrap_or_default();

    // Status aktualisieren
    {
        let mut st = ctx.status.lock().await;
        if p.index < st.sources.len() {
            st.sources[p.index] = if uri.is_empty() {
                None
            } else {
                Some(uri.clone())
            };
            let _ = ctx.tx.send(st.clone());
        }
    }

    // Optional: sofort Pipeline-Rebuild mit aktuellen Settings
    let out_rtmp = crate::db::settings_get(&ctx.db, "output.rtmp")
        .await
        .ok()
        .flatten();

    let audio_dev = crate::db::settings_get(&ctx.db, "audio.device")
        .await
        .ok()
        .flatten();


    // --- Recording options ---
    let rec_enabled = crate::db::settings_get(&ctx.db, "record.enabled")
        .await
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false);

    let rec_opts: Option<RecordOpts> = if rec_enabled {
        // directory
        let rec_dir = crate::db::settings_get(&ctx.db, "record.dir")
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                format!(
                    "{}/.local/share/openresquesim/recordings",
                    std::env::var("HOME").unwrap_or_else(|_| ".".into())
                )
            });

        // file prefix / basename
        let rec_base = crate::db::settings_get(&ctx.db, "record.prefix")
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "rec".into());

        // segment length (seconds) -> split_ms (milliseconds)
        let rec_segment_ms: i64 = crate::db::settings_get(&ctx.db, "record.segment_secs")
            .await
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u32>().ok())
            .map(|secs| secs as i64 * 1000)
            .unwrap_or(300_000); // default 5min

        // optional rotation count
        let rec_max_files: i32 = crate::db::settings_get(&ctx.db, "record.max_files")
            .await
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);

        // container format from app_config (returns "mp4" | "mkv" | "mov")
        let rec_container = app_config::record_container(&ctx.db).await; // already "mp4" | "mkv" | "mov"

        Some(RecordOpts {
            enabled: true,
            directory: rec_dir,
            basename: rec_base,
            container: rec_container,
            split_ms: rec_segment_ms,
            max_files: rec_max_files,
            audio_dev: audio_dev.clone(),
        })
    } else {
        None
    };

    let status_clone = ctx.status.lock().await.clone();
    {
        let mut pm = ctx.pipeline.lock().await;
        if let Err(e) = pm.rebuild(
            &status_clone,
            out_rtmp.as_deref(),
            audio_dev.as_deref(),
            rec_opts,
        ) {
            eprintln!("camera_add_simple: pipeline rebuild failed: {e:?}");
        }
    }

    // Hier die geklonten Werte verwenden (keine Move-Probleme mehr)
    Ok(axum::Json(serde_json::json!({
        "ok": true,
        "index": p.index,
        "uri": uri,
        "rtsp": rtsp,
        "rtmp": rtmp,
    })))
}

#[derive(serde::Deserialize)]
pub struct CameraUpdatePayload {
    pub name: Option<String>,
    pub url: Option<String>,
    pub mjpeg_url: Option<String>,
    pub fov_deg: Option<f64>,
    pub enabled: Option<bool>,
    pub brand: Option<String>,
    pub ptz_capable: Option<bool>,
    // Leer/weggelassen = Zugangsdaten unverändert lassen (Passwortfeld wird beim
    // Bearbeiten im Frontend nicht vorausgefüllt, siehe options.js).
    pub vapix_username: Option<String>,
    pub vapix_password: Option<String>,
    pub vapix_port: Option<i64>,
}

pub async fn cameras_update_handler(
    State(ctx): State<AppCtx>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<CameraUpdatePayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    ensure_admin(&user)?;
    // Aktuelle Kamera holen
    let cam = crate::db::camera_get(&ctx.db, id).await.map_err(|e| {
        error!(?e, "get camera before update");
        StatusCode::NOT_FOUND
    })?;

    let name = body.name.unwrap_or(cam.name);
    let url = body.url.unwrap_or(cam.url);
    let mjpeg_url = if let Some(raw) = body.mjpeg_url {
        let trimmed = raw.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    } else {
        cam.mjpeg_url
    };
    let fov = body.fov_deg.or(cam.fov_deg);
    let enabled = body.enabled.unwrap_or(cam.enabled != 0);
    let brand = body.brand.unwrap_or(cam.brand);
    let ptz_capable = body.ptz_capable.unwrap_or(cam.ptz_capable != 0);
    // Leeres/weggelassenes Feld lässt die gespeicherten Zugangsdaten unangetastet.
    let vapix_username = body
        .vapix_username
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or(cam.vapix_username);
    let vapix_password = body
        .vapix_password
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or(cam.vapix_password);
    let vapix_port = body.vapix_port.or(cam.vapix_port);

    let ok = crate::db::cameras_update(
        &ctx.db,
        id,
        &name,
        &url,
        mjpeg_url.as_deref(),
        fov,
        enabled,
        &brand,
        ptz_capable,
        vapix_username.as_deref(),
        vapix_password.as_deref(),
        vapix_port,
    )
    .await
    .map_err(|e| {
        error!(?e, "update camera");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if !ok {
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(Json(json!({ "ok": true })))
}

pub async fn cameras_delete_handler(
    State(ctx): State<AppCtx>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    ensure_admin(&user)?;
    crate::db::cameras_delete(&ctx.db, id).await.map_err(|e| {
        error!(id, ?e, "delete camera");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct CameraCreatePayload {
    pub name: String,
    pub url: String,
    pub mjpeg_url: Option<String>,
    pub fov_deg: Option<f64>,
    // `enabled` lassen wir im Payload weg – DB hat Default.
    // Wenn du magst, kannst du es hier ergänzen und später per UPDATE setzen.
    pub brand: Option<String>,
    pub ptz_capable: Option<bool>,
    pub vapix_username: Option<String>,
    pub vapix_password: Option<String>,
    pub vapix_port: Option<i64>,
}

pub async fn cameras_create_handler(
    State(ctx): State<AppCtx>,
    user: AuthUser,
    Json(body): Json<CameraCreatePayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    ensure_admin(&user)?;
    // 1) Insert mit deiner vorhandenen Funktion
    let mjpeg_url = body
        .mjpeg_url
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let vapix_username = body
        .vapix_username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let vapix_password = body
        .vapix_password
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let id = crate::db::cameras_add(
        &ctx.db,
        &body.name,
        &body.url,
        mjpeg_url.as_deref(),
        body.fov_deg,
        crate::db::CameraPtzFields {
            brand: body.brand.as_deref(),
            ptz_capable: body.ptz_capable,
            vapix_username,
            vapix_password,
            vapix_port: body.vapix_port,
        },
    )
    .await
    .map_err(|e| {
        error!(?e, "insert camera");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // 2) Direkt danach die frisch angelegte Kamera lesen
    let cam = crate::db::camera_get(&ctx.db, id).await.map_err(|e| {
        error!(?e, "get camera after insert");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // 3) JSON so zurückgeben, wie es die Liste/Update-Logik erwartet
    Ok(Json(json!({
        "id":       cam.id,
        "name":     cam.name,
        "url":      cam.url,
        "mjpeg_url": cam.mjpeg_url,
        "fov_deg":  cam.fov_deg,
        "enabled":  cam.enabled != 0,
        "brand":    cam.brand,
        "ptz_capable": cam.ptz_capable != 0,
    })))
}

// ---- Camera PTZ (Pan/Tilt/Zoom) ----
// Zugriff: jeder eingeloggte User (kein ensure_admin) — Einrichten/Löschen der
// Kamera bleibt admin-only, aber das Bedienen einer bereits eingerichteten
// PTZ-Kamera soll jedem Nutzer möglich sein.

#[derive(Deserialize)]
pub struct PtzMovePayload {
    pub pan: i32,
    pub tilt: i32,
    pub zoom: i32,
}

pub async fn camera_ptz_move(
    State(ctx): State<AppCtx>,
    _user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<PtzMovePayload>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let cam = crate::db::camera_get(&ctx.db, id).await.map_err(|e| {
        error!(?e, "get camera for ptz");
        (StatusCode::NOT_FOUND, Json(json!({"message": "Kamera nicht gefunden."})))
    })?;

    let pan = body.pan.clamp(-100, 100);
    let tilt = body.tilt.clamp(-100, 100);
    let zoom = body.zoom.clamp(-100, 100);

    crate::helperscripts::ptz::ptz_move(&cam, pan, tilt, zoom)
        .await
        .map_err(|e| {
            use crate::helperscripts::ptz::PtzError;
            let status = match e {
                PtzError::Unsupported => StatusCode::NOT_IMPLEMENTED,
                PtzError::NotConfigured => StatusCode::PRECONDITION_FAILED,
                PtzError::Network(_) | PtzError::Http(_) => StatusCode::BAD_GATEWAY,
            };
            (status, Json(json!({"message": e.to_string()})))
        })?;

    Ok(Json(json!({ "ok": true })))
}

// ---- Camera Snapshot (On-Demand) ----

#[derive(Deserialize)]
pub struct CameraSnapshotQuery {
    pub fresh: Option<bool>,
}

/// GET /api/cameras/:id/snapshot  -> liefert vorhandenes Bild oder erzeugt bei Bedarf
pub async fn camera_snapshot(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
    Query(q): Query<CameraSnapshotQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let fresh = q.fresh.unwrap_or(false);
    let bytes = preview_helper::get_or_create_snapshot(&ctx.db, id, fresh)
        .await
        .map_err(|e| {
            error!(id, ?e, "camera snapshot failed");
            StatusCode::BAD_GATEWAY
        })?;
    Ok(([(CONTENT_TYPE, "image/jpeg")], bytes))
}

/// POST /api/cameras/:id/snapshot -> erzwingt einen frischen Snapshot
pub async fn camera_snapshot_force(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, StatusCode> {
    let bytes = preview_helper::get_or_create_snapshot(&ctx.db, id, true)
        .await
        .map_err(|e| {
            error!(id, ?e, "camera snapshot force failed");
            StatusCode::BAD_GATEWAY
        })?;
    Ok(([(CONTENT_TYPE, "image/jpeg")], bytes))
}
// --------- VIEWER-LISTE (für Options-Seite) ----------

// --------- AUDIO-QUELLEN (Stub) ----------
pub async fn audio_sources_list(State(ctx): State<AppCtx>) -> Json<serde_json::Value> {
    let resp = crate::host::audio_inputs_handler(State(ctx)).await;
    // resp: Json<AudioInputDevicesResponse>
    serde_json::json!({ "sources": resp.0.devices }).into()
}

#[derive(serde::Deserialize)]
pub struct AudioVolumePayload {
    #[allow(dead_code)]
    #[serde(rename = "inputName")]
    pub input_name: Option<String>,
    #[serde(rename = "volumePercent")]
    pub volume_percent: Option<f64>,
}

#[derive(serde::Deserialize)]
pub struct AudioTogglePayload {
    #[allow(dead_code)]
    #[serde(rename = "inputName")]
    pub input_name: Option<String>,
}

// ---- Audio Input Definitions (global) ----------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioInputDef {
    pub id: i64,
    pub device: String,
    pub label: Option<String>,
    pub gain: Option<f64>,
    /// Hardware-Kanäle (1-basiert), leer = alle
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<u32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioInputDefIncoming {
    pub id: Option<i64>,
    pub device: String,
    pub label: Option<String>,
    pub gain: Option<f64>,
    #[serde(default)]
    pub channels: Option<Vec<u32>>,
}

fn settings_audio_inputs_key() -> &'static str {
    "audio.inputs"
}

async fn load_audio_inputs(db: &sqlx::SqlitePool) -> anyhow::Result<Vec<AudioInputDef>> {
    let raw = crate::db::settings_get(db, settings_audio_inputs_key())
        .await
        .unwrap_or(None)
        .unwrap_or_default();
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let parsed: Vec<AudioInputDef> = serde_json::from_str(&raw).unwrap_or_default();
    Ok(parsed)
}

async fn save_audio_inputs(db: &sqlx::SqlitePool, list: &[AudioInputDef]) -> anyhow::Result<()> {
    let raw = serde_json::to_string(list).unwrap_or_else(|_| "[]".into());
    crate::db::settings_set(db, settings_audio_inputs_key(), &raw).await?;
    Ok(())
}

pub async fn audio_inputs_defs_get(
    State(ctx): State<AppCtx>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let list = load_audio_inputs(&ctx.db).await.map_err(|e| {
        error!(?e, "load audio inputs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(json!({ "audio_inputs": list })))
}

#[derive(Deserialize)]
pub struct AudioInputsUpsertPayload {
    pub audio_inputs: Vec<AudioInputDefIncoming>,
}

pub async fn audio_inputs_defs_set(
    State(ctx): State<AppCtx>,
    Json(body): Json<AudioInputsUpsertPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let existing = load_audio_inputs(&ctx.db).await.unwrap_or_default();
    let mut next_id = existing.iter().map(|a| a.id).max().unwrap_or(0) + 1;

    let mut out: Vec<AudioInputDef> = Vec::new();
    for item in body.audio_inputs {
        if item.device.trim().is_empty() {
            continue;
        }
        let id = item.id.unwrap_or_else(|| {
            let nid = next_id;
            next_id += 1;
            nid
        });
        let channels = item.channels.as_ref().map(|c| {
            let mut v: Vec<u32> = c.iter().copied().filter(|n| *n >= 1 && *n <= 64).collect();
            v.dedup();
            v
        }).filter(|v| !v.is_empty());
        out.push(AudioInputDef {
            id,
            device: item.device.trim().to_string(),
            label: item.label.clone(),
            gain: item.gain,
            channels,
        });
    }

    if let Err(e) = save_audio_inputs(&ctx.db, &out).await {
        error!(?e, "save audio inputs");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    Ok(Json(json!({ "ok": true, "audio_inputs": out })))
}

#[derive(serde::Serialize)]
pub struct AudioStateResponse {
    #[serde(rename = "volumePercent")]
    pub volume_percent: f64,
    pub muted: bool,
}

pub async fn audio_state(State(ctx): State<AppCtx>) -> Json<AudioStateResponse> {
    let pm = ctx.pipeline.lock().await;
    let (vol, muted) = pm.audio_state();
    Json(AudioStateResponse {
        volume_percent: vol,
        muted,
    })
}

pub async fn audio_volume_set(
    axum::extract::State(ctx): axum::extract::State<AppCtx>,
    axum::Json(p): axum::Json<AudioVolumePayload>,
) -> Result<axum::Json<serde_json::Value>, axum::http::StatusCode> {
    let target = p.volume_percent.unwrap_or(100.0);
    let mut pm = ctx.pipeline.lock().await;
    let applied = pm
        .set_audio_volume(target)
        .map_err(|_| axum::http::StatusCode::SERVICE_UNAVAILABLE)?;

    let (_, muted) = pm.audio_state();
    Ok(axum::Json(serde_json::json!({
        "ok": true,
        "volumePercent": applied,
        "muted": muted,
    })))
}

pub async fn audio_mute_toggle(
    axum::extract::State(ctx): axum::extract::State<AppCtx>,
    axum::Json(_p): axum::Json<AudioTogglePayload>,
) -> Result<axum::Json<serde_json::Value>, axum::http::StatusCode> {
    let mut pm = ctx.pipeline.lock().await;
    let muted = pm
        .toggle_audio_mute()
        .map_err(|_| axum::http::StatusCode::SERVICE_UNAVAILABLE)?;
    let (vol, _) = pm.audio_state();
    Ok(axum::Json(serde_json::json!({
        "ok": true,
        "muted": muted,
        "volumePercent": vol,
    })))
}

// ---------- FS-Utils ----------

async fn recordings_base_dir(ctx: &AppCtx) -> String {
    crate::db::settings_get(&ctx.db, "record.dir")
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            format!(
                "{}/.local/share/openresquesim/recordings",
                std::env::var("HOME").unwrap_or_else(|_| ".".into())
            )
        })
}

async fn remove_path_tolerant<P: AsRef<FsPath>>(p: P) -> Result<(), std::io::Error> {
    let p = p.as_ref();
    match fs::metadata(p).await {
        Ok(md) => {
            if md.is_dir() {
                fs::remove_dir_all(p).await
            } else {
                fs::remove_file(p).await
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

// ---------- API ----------

pub async fn list_sessions(
    State(ctx): State<AppCtx>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let rows = crate::db::sessions_list(&ctx.db).await.map_err(|e| {
        error!(?e, "list sessions");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let user = auth_from_headers(&ctx, &headers);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_else(|_| std::time::Duration::from_secs(0))
        .as_secs() as i64;

    let sessions: Vec<_> = rows
        .into_iter()
        .filter(|r| session_visible_for_user(user.as_ref(), r, now))
        .map(|r| {
            json!({
                "id": r.id,
                "name": r.name,
                "record_is_active": r.record_is_active,
                "start_time": r.start_time,
                "stop_time": r.stop_time,
                "length": r.length,
                "filename": r.filename,
                "snapshot": format!("/api/sessions/{}/snapshot", r.id),
            })
        })
        .collect();

    Ok(Json(json!({ "sessions": sessions })))
}

fn first_recording_candidate(
    rec_dir: &std::path::Path,
    sid: i64,
    filename_from_db: Option<String>,
) -> Option<std::path::PathBuf> {
    if let Some(name) = filename_from_db {
        let p = rec_dir.join("vod").join(name);
        if p.exists() {
            return Some(p);
        }
    }

    let sess_dir = rec_dir.join(sid.to_string());
    if sess_dir.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(&sess_dir)
            .ok()?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            if name.ends_with(".mkv") || name.ends_with(".mp4") {
                return Some(e.path());
            }
        }
    }

    let direct = rec_dir.join(format!("session-{}.mkv", sid));
    if direct.exists() {
        return Some(direct);
    }

    None
}

pub async fn session_snapshot(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    ensure_session_visible(&ctx, &headers, id).await?;
    let dirs = crate::db::app_dirs();
    let rec_dir = dirs.rec_dir;
    let sess_dir = rec_dir.join(id.to_string());
    let snapshot_path = sess_dir.join("snapshot.jpg");

    if let Ok(bytes) = tokio::fs::read(&snapshot_path).await {
        return Ok(([(CONTENT_TYPE, "image/jpeg")], bytes));
    }

    let _ = std::fs::create_dir_all(&sess_dir);

    let filename_from_db = crate::db::sim_session_filename(&ctx.db, id)
        .await
        .ok()
        .flatten();
    let Some(input) = first_recording_candidate(&rec_dir, id, filename_from_db) else {
        return Err(StatusCode::NOT_FOUND);
    };

    let out_str = snapshot_path.to_string_lossy().to_string();
    let input_clone = input.clone();
    let status = spawn_blocking(move || {
        Command::new("ffmpeg")
            .arg("-y")
            .arg("-ss")
            .arg("0")
            .arg("-i")
            .arg(input_clone)
            .arg("-frames:v")
            .arg("1")
            .arg("-q:v")
            .arg("2")
            .arg(&out_str)
            .status()
    })
    .await
    .ok()
    .and_then(|r| r.ok());

    if status.map(|s| s.success()).unwrap_or(false) {
        if let Ok(bytes) = tokio::fs::read(&snapshot_path).await {
            return Ok(([(CONTENT_TYPE, "image/jpeg")], bytes));
        }
    }

    Err(StatusCode::NOT_FOUND)
}

pub async fn delete_one_session(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use serde_json::json;
    delete_session_full(&ctx, id).await?;

    Ok(Json(json!({ "ok": true, "deleted": 1, "id": id })))
}

async fn delete_session_full(ctx: &AppCtx, id: i64) -> Result<(), StatusCode> {
    use std::path::PathBuf;
    use tracing::{error, warn};

    // 1️⃣ Session lookup (brauchen wir für Dateipfad oder Ordner)
    let Some(row) = crate::db::session_by_id(&ctx.db, id).await.map_err(|e| {
        error!(?e, "fetch session by id");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    else {
        return Err(StatusCode::NOT_FOUND);
    };

    // 1b️⃣ VOD-Dateien zur Session löschen (falls vorhanden)
    let vod_items = crate::db::vod_list_by_session(&ctx.db, id)
        .await
        .unwrap_or_default();
    for item in vod_items {
        if let Some(fp) = item.file_path.as_ref() {
            if let Err(e) = remove_path_tolerant(fp).await {
                warn!(id, file=%fp, ?e, "failed to remove VOD file");
            }
        } else if let Some(url) = item.url.as_ref() {
            if url.starts_with("/vod/") {
                let rel = url.trim_start_matches('/');
                let base = recordings_base_dir(&ctx).await;
                let p = std::path::Path::new(&base).join(rel);
                if let Err(e) = remove_path_tolerant(&p).await {
                    warn!(id, file=%p.display(), ?e, "failed to remove VOD file from url");
                }
                let fallback_base = crate::db::app_dirs().rec_dir;
                let p2 = fallback_base.join(rel);
                if let Err(e) = remove_path_tolerant(&p2).await {
                    warn!(id, file=%p2.display(), ?e, "failed to remove VOD file from url (fallback)");
                }
            }
        }
    }

    // 2️⃣ Dateipfad löschen (falls filename gesetzt)
    if let Some(fname) = row.filename.as_deref() {
        let fname_path = std::path::Path::new(fname);
        if let Err(e) = remove_path_tolerant(fname).await {
            warn!(id, ?fname, ?e, "failed to remove session file");
        }
        // Falls nur ein relativer Name in der DB steht
        if fname_path.is_relative() {
            let base = recordings_base_dir(&ctx).await;
            let p = std::path::Path::new(&base).join(fname_path);
            if let Err(e) = remove_path_tolerant(&p).await {
                warn!(id, file=%p.display(), ?e, "failed to remove session file (record.dir)");
            }
            let fallback_base = crate::db::app_dirs().rec_dir;
            let p2 = fallback_base.join(fname_path);
            if let Err(e) = remove_path_tolerant(&p2).await {
                warn!(id, file=%p2.display(), ?e, "failed to remove session file (fallback)");
            }
        }
    }

    // 3️⃣ Session-Ordner löschen (immer versuchen; mehrere mögliche Basispfade)
    let mut bases = vec![recordings_base_dir(&ctx).await];
    let fallback_base = crate::db::app_dirs().rec_dir;
    if !bases.iter().any(|b| b == fallback_base.to_string_lossy().as_ref()) {
        bases.push(fallback_base.to_string_lossy().to_string());
    }
    if let Some(fname) = row.filename.as_deref() {
        if let Some(parent) = std::path::Path::new(fname).parent() {
            let p = parent.to_string_lossy().to_string();
            if !bases.iter().any(|b| b == &p) {
                bases.push(p);
            }
        }
    }
    for base in bases {
        let candidates = [
            PathBuf::from(&base).join(id.to_string()),
            PathBuf::from(&base).join(format!("session-{}", id)),
            PathBuf::from(&base).join(format!("session_{id}")),
        ];
        for session_dir in candidates {
            if let Err(e) = remove_path_tolerant(&session_dir).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!(id, dir=?session_dir, ?e, "failed to remove session directory");
                }
            }
        }
    }

    // 4️⃣ DB (Marker + Session) löschen
    crate::db::delete_session_in_txn(&ctx.db, id)
        .await
        .map_err(|e| {
            error!(id, ?e, "failed to delete DB rows (markers/sessions)");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(())
}

pub async fn delete_all_sessions(
    State(ctx): State<AppCtx>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use serde_json::json;
    use tracing::error;
    let mut deleted_fs = 0usize;
    let sessions = crate::db::sessions_list(&ctx.db).await.unwrap_or_default();
    for s in sessions {
        if let Some(id) = Some(s.id) {
            if delete_session_full(&ctx, id).await.is_ok() {
                deleted_fs += 1;
            }
        }
    }

    crate::db::delete_all_sessions_in_txn(&ctx.db)
        .await
        .map_err(|e| {
            error!(?e, "failed to delete all sessions/markers in DB");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(json!({ "ok": true, "deleted_fs": deleted_fs })))
}

//--Viewers--//

pub async fn viewers_list(
    State(ctx): State<AppCtx>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let rows = crate::db::viewers_list(&ctx.db).await.map_err(|e| {
        error!(?e, "list viewers");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let map = ctx.viewer_channels.lock().await;

    let enriched: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|row| {
            let id = row.id;
            let online = map
                .get(&id)
                .map(|list| list.iter().any(|tx| !tx.is_closed()))
                .unwrap_or(false);

            let mut value = serde_json::to_value(row).unwrap_or_else(|_| json!({}));
            if let serde_json::Value::Object(ref mut obj) = value {
                obj.insert("online".into(), json!(online));
            }
            value
        })
        .collect();

    Ok(Json(json!({ "viewers": enriched })))
}

pub async fn viewers_create(
    State(ctx): State<AppCtx>,
    Json(body): Json<crate::db::ViewerCreate>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // (optional) Validierung type
    let allowed = ["webrtc", "rtsp", "custom"];
    if !allowed.contains(&body.r#type.as_str()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let id = crate::db::viewer_create(&ctx.db, body).await.map_err(|e| {
        error!(?e, "create viewer");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(json!({ "ok": true, "id": id })))
}

pub async fn viewers_patch(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
    Json(body): Json<crate::db::ViewerPatch>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    crate::db::viewer_update(&ctx.db, id, body)
        .await
        .map_err(|e| {
            error!(id, ?e, "patch viewer");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn viewers_delete(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    crate::db::viewer_delete(&ctx.db, id).await.map_err(|e| {
        error!(id, ?e, "delete viewer");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(json!({ "ok": true })))
}

// POST /api/viewers/:id/cmd
pub async fn viewers_cmd(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
    Json(cmd): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    // cmd ist hier ein serde_json::Value, egal ob {cmd:"..."} oder {action:"..."}
    send_viewer_command(&ctx, id, cmd).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct ViewerReportPayload {
    pub session_id: Option<i64>,
    pub note: Option<String>,
    pub logs: Option<Vec<String>>,
    pub url: Option<String>,
}

#[derive(Deserialize)]
pub struct ViewerReportAckPayload {
    pub session_id: Option<i64>,
    pub ack_by: Option<String>,
}

// POST /api/viewers/:id/report
pub async fn viewers_report(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
    Json(body): Json<ViewerReportPayload>,
) -> Result<Json<Value>, StatusCode> {
    let msg = json!({
        "type": "viewer_report",
        "viewer_id": id,
        "session_id": body.session_id,
        "note": body.note,
        "logs": body.logs,
        "url": body.url,
        "ts": Utc::now().to_rfc3339(),
    });
    broadcast_ui(&ctx, msg).await;
    Ok(Json(json!({ "ok": true })))
}

// POST /api/viewers/:id/report_ack
pub async fn viewers_report_ack(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
    Json(body): Json<ViewerReportAckPayload>,
) -> Result<Json<Value>, StatusCode> {
    let target_id = match crate::db::viewer_id_by_url(&ctx.db, &format!("/viewer/{id}")).await {
        Ok(Some(db_id)) => db_id,
        _ => id,
    };
    let cmd = json!({
        "action": "report_ack",
        "session_id": body.session_id,
        "ack_by": body.ack_by,
        "ts": Utc::now().to_rfc3339(),
    });
    send_viewer_command(&ctx, target_id, cmd).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize, Default)]
pub struct ViewerWsQuery {
    pub device_key: Option<String>,
}

pub async fn viewers_ws(
    State(ctx): State<AppCtx>,
    Path(url_id): Path<i64>, // ID aus der URL /viewer/:id
    Query(q): Query<ViewerWsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Nur gekoppelte Geräte: sonst weder DB-Eintrag noch Befehle/Tickets, die Seite zeigt
    // „Bitte koppeln“ (siehe pairing.rs). Früher wurde jede Verbindung angenommen und
    // automatisch als Viewer angelegt.
    if !crate::pairing::device_allowed(&ctx.db, url_id, q.device_key.as_deref()).await {
        return ws.on_upgrade(move |mut socket| async move {
            let _ = socket
                .send(WsMessage::Text(json!({ "cmd": "pair_required", "viewer_no": url_id }).to_string()))
                .await;
            let _ = socket.close().await;
        });
    }
    // URL-basierten Viewer in der DB registrieren oder updaten
    let name = format!("Viewer-{url_id}");
    let url = format!("/viewer/{url_id}");

    let db_id = match crate::db::viewer_register_broadcast(&ctx.db, &name, &url).await {
        Ok(id) => {
            tracing::info!("📡 Viewer-WS: URL-ID={url_id} → DB-ID={id}");
            id
        }
        Err(e) => {
            tracing::warn!("auto-register viewer (url_id={url_id}) failed: {e:?}");
            // Fallback: wir benutzen die URL-ID als Key – dann passen die IDs
            // zwar nicht zur DB, aber wenigstens funktioniert WS.
            url_id
        }
    };

    ws.on_upgrade(move |socket| viewer_ws_connection(socket, ctx, db_id))
}

async fn viewer_ws_connection(socket: WebSocket, ctx: AppCtx, db_id: i64) {
    tracing::info!("🧷 Viewer WS verbunden: db_id={db_id}");

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    {
        let mut map = ctx.viewer_channels.lock().await;
        map.entry(db_id).or_default().push(tx.clone());
    }

    // Läuft gerade eine Durchsage an die Viewer, sofort mithören (sonst hörte ein Viewer,
    // der sich währenddessen verbindet oder neu lädt, nichts bis zur nächsten Durchsage)
    if crate::voice::viewer_paging_active() {
        let mut cmd = json!({"action":"paging_start","ws":"/ws/voice/viewer"});
        crate::stream_auth::attach_ticket(&ctx, &mut cmd);
        let _ = tx.send(cmd.to_string());
    }

    // Autorejoin: wenn Session aktiv und dieser Viewer in der aktiven Liste ist → sofort show_live schicken
    if let Ok(Some(active_sid)) = crate::db::sim_active_session_id(&ctx.db).await {
        if let Ok(Some(active_list)) = crate::db::settings_get(&ctx.db, "sim.active_viewers").await
        {
            if let Ok(ids) = serde_json::from_str::<Vec<i64>>(&active_list) {
                if ids.iter().any(|id| *id == db_id) {
                    let ws_path = format!("/api/webrtc/{active_sid}");
                    let reset_cmd = json!({ "cmd": "reset_webrtc" });
                    let live_cmd = json!({ "cmd": "show_live", "url": ws_path });
                    crate::routes::send_viewer_command(&ctx, db_id, reset_cmd).await;
                    crate::routes::send_viewer_command(&ctx, db_id, live_cmd).await;
                    tracing::info!(
                        "Viewer db_id={db_id}: Autorejoin auf aktive Session {active_sid}"
                    );
                }
            }
        }
    }

    let (mut ws_tx, mut ws_rx) = socket.split();

    let send_task = tokio::spawn(async move {
        while let Some(payload) = rx.recv().await {
            if ws_tx.send(WsMessage::Text(payload)).await.is_err() {
                break;
            }
        }
    });

    while let Some(msg) = ws_rx.next().await {
        match msg {
            Ok(WsMessage::Text(text)) => {
                tracing::debug!("Viewer db_id={db_id} sendet (ignoriert): {text}");
            }
            Ok(WsMessage::Close(_)) => {
                tracing::info!("Viewer db_id={db_id} schließt Verbindung");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("Viewer db_id={db_id} WS-Fehler: {e}");
                break;
            }
        }
    }

    // Verbindung zu: Sendeschleife beenden und genau diesen Kanal austragen. Früher
    // wurden nur „geschlossene“ Kanäle entfernt – dieser galt aber nicht als geschlossen,
    // weil die Sendeschleife noch auf ihn wartete. Der Viewer blieb so dauerhaft „online“
    // und jede getrennte Verbindung blieb als Leiche in viewer_channels.
    send_task.abort();
    {
        let mut map = ctx.viewer_channels.lock().await;
        if let Some(list) = map.get_mut(&db_id) {
            list.retain(|s| !s.same_channel(&tx) && !s.is_closed());
            if list.is_empty() {
                map.remove(&db_id);
            }
        }
    }
    tracing::info!("🔌 Viewer WS getrennt: db_id={db_id}");
}

pub async fn send_viewer_command(ctx: &AppCtx, id: i64, mut cmd: serde_json::Value) {
    crate::stream_auth::attach_ticket(ctx, &mut cmd);
    let payload = match serde_json::to_string(&cmd) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("viewer({id}) command serialize error: {e}");
            return;
        }
    };

    let mut remove_entry = false;
    let mut delivered = false;

    {
        let mut map = ctx.viewer_channels.lock().await;
        if let Some(list) = map.get_mut(&id) {
            // an alle Sender schicken; geschlossene rausschmeißen
            list.retain(|tx| {
                let ok = tx.send(payload.clone()).is_ok();
                if ok {
                    delivered = true;
                }
                ok
            });
            if list.is_empty() {
                remove_entry = true;
            }
        }
        if remove_entry {
            map.remove(&id);
        }
    }

    if !delivered {
        tracing::warn!(
            "viewer({id}) command dropped (no active WS connection?) cmd={}",
            payload
        );
    } else {
        tracing::debug!("viewer({id}) command delivered cmd={}", payload);
    }
}

/// Schickt ein Kommando an alle aktuell verbundenen Viewer-WS-Clients.
pub async fn broadcast_viewer_command(ctx: &AppCtx, mut cmd: serde_json::Value) {
    crate::stream_auth::attach_ticket(ctx, &mut cmd);
    let payload = match serde_json::to_string(&cmd) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("viewer(broadcast) command serialize error: {e}");
            return;
        }
    };

    let mut delivered = 0usize;
    let mut to_remove: Vec<i64> = Vec::new();

    {
        let mut map = ctx.viewer_channels.lock().await;
        for (id, list) in map.iter_mut() {
            list.retain(|tx| {
                let ok = tx.send(payload.clone()).is_ok();
                if ok {
                    delivered += 1;
                }
                ok
            });
            if list.is_empty() {
                to_remove.push(*id);
            }
        }
        for id in to_remove {
            map.remove(&id);
        }
    }

    if delivered == 0 {
        tracing::warn!(
            "viewer(broadcast) command dropped (no active WS connection?) cmd={}",
            payload
        );
    } else {
        tracing::debug!(
            "viewer(broadcast) command delivered to {delivered} clients cmd={}",
            payload
        );
    }
}

// --------------------------------------------------------------
// Rooms API
// --------------------------------------------------------------

pub async fn rooms_list(State(ctx): State<AppCtx>) -> Result<Json<serde_json::Value>, StatusCode> {
    let rows = crate::db::rooms_list(&ctx.db).await.map_err(|e| {
        error!(?e, "list rooms");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(json!({ "rooms": rows })))
}

pub async fn rooms_get(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
) -> Result<Json<crate::db::RoomRow>, StatusCode> {
    let room = crate::db::room_get(&ctx.db, id).await.map_err(|e| {
        error!(id, ?e, "get room");
        StatusCode::NOT_FOUND
    })?;
    Ok(Json(room))
}

pub async fn rooms_create(
    State(ctx): State<AppCtx>,
    Json(body): Json<crate::db::RoomCreate>,
) -> Result<Json<crate::db::RoomRow>, StatusCode> {
    let id = crate::db::room_create(&ctx.db, body).await.map_err(|e| {
        error!(?e, "create room");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let room = crate::db::room_get(&ctx.db, id).await.map_err(|e| {
        error!(id, ?e, "get room after create");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(room))
}

pub async fn rooms_patch(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
    Json(body): Json<crate::db::RoomPatch>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let new_speakers = body.speakers.clone().map(|sp| crate::voice::parse_room_speakers(sp.as_deref()));
    crate::db::room_update(&ctx.db, id, body)
        .await
        .map_err(|e| {
            error!(id, ?e, "patch room");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    // Läuft gerade eine Durchsage an diesen Raum, Lautsprecher sofort angleichen
    if let Some(sp) = new_speakers {
        crate::voice::room_speakers_changed(id, sp).await;
    }

    Ok(Json(json!({ "ok": true })))
}

pub async fn rooms_delete(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    crate::db::room_delete(&ctx.db, id).await.map_err(|e| {
        error!(id, ?e, "delete room");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(json!({ "ok": true })))
}

// --------------------------------------------------------------
// Health
// --------------------------------------------------------------
pub async fn health(State(ctx): State<AppCtx>) -> Json<serde_json::Value> {
    // Pipeline-Infos
    let (pipeline_state, layout_slots, fps, dropped_frames, vlc_port) = {
        let pm = ctx.pipeline.lock().await;
        if let Some(p) = &pm.pipeline {
            let state = p.current_state();
            let (fps, drop) = pm.metrics_snapshot();
            let vlc = {
                let enabled = std::env::var("ENABLE_VLC_STREAM")
                    .map(|v| {
                        v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes")
                    })
                    .unwrap_or(false);
                if enabled {
                    std::env::var("VLC_TCP_PORT")
                        .ok()
                        .and_then(|s| s.parse::<u16>().ok())
                        .unwrap_or(9100)
                        .into()
                } else {
                    Option::<u16>::None
                }
            };
            (
                format!("{:?}", state),
                pm.current_layout_slots(),
                fps,
                drop,
                vlc,
            )
        } else {
            ("Null".to_string(), pm.current_layout_slots(), 0.0, 0, None)
        }
    };

    let active_session = crate::db::sim_active_session_id(&ctx.db)
        .await
        .ok()
        .flatten()
        .is_some();

    let viewer_map = ctx.viewer_channels.lock().await;
    let viewer_connections = viewer_map.values().map(|v| v.len()).sum::<usize>();

    // aktive Viewer-Liste aus Settings + Online-Status aus viewer_channels
    let active_viewers: Vec<serde_json::Value> = {
        let raw = crate::db::settings_get(&ctx.db, "sim.active_viewers")
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "[]".into());
        let parsed: Vec<i64> = serde_json::from_str(&raw).unwrap_or_default();
        let mut res = Vec::new();
        for vid in parsed {
            let online = viewer_map
                .get(&vid)
                .map(|list| list.iter().any(|tx| !tx.is_closed()))
                .unwrap_or(false);
            // Name nachschlagen (best effort)
            let name = crate::db::viewer_get(&ctx.db, vid)
                .await
                .ok()
                .map(|v| v.alias.unwrap_or(v.name));
            res.push(json!({
                "id": vid,
                "name": name,
                "online": online
            }));
        }
        res
    };

    Json(json!({
        "ok": true,
        "active_session": active_session,
        "viewer_connections": viewer_connections,
        "active_viewers": active_viewers,
        "cpu_percent": serde_json::Value::Null,
        "mem_total": serde_json::Value::Null,
        "mem_used": serde_json::Value::Null,
        "pipeline_state": pipeline_state,
        "layout_slots": layout_slots,
        "fps": fps,
        "dropped_frames": dropped_frames,
        "vlc_port": vlc_port,
    }))
}

// --------------------------------------------------------------
// VLC-Notfallstream toggeln (setzt Env und triggert Rebuild)
#[derive(Deserialize)]
pub struct VlcToggle {
    pub enabled: bool,
}

pub async fn vlc_toggle(
    State(ctx): State<AppCtx>,
    Json(p): Json<VlcToggle>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if p.enabled {
        env::set_var("ENABLE_VLC_STREAM", "true");
    } else {
        env::remove_var("ENABLE_VLC_STREAM");
    }

    // Pipeline neu bauen, damit VLC-Zweig entsprechend gesetzt ist
    let st = ctx.status.lock().await.clone();
    let out_rtmp = crate::db::settings_get(&ctx.db, "output.rtmp")
        .await
        .ok()
        .flatten();
    let audio_dev = crate::db::settings_get(&ctx.db, "audio.device")
        .await
        .ok()
        .flatten();

    let rec_enabled = crate::db::settings_get(&ctx.db, "record.enabled")
        .await
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false);

    let rec_opts = if rec_enabled {
        let dirs = crate::db::app_dirs();
        let dir = crate::db::settings_get(&ctx.db, "record.dir")
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| dirs.rec_dir.to_string_lossy().to_string());
        let basename = crate::db::settings_get(&ctx.db, "record.prefix")
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "rec".into());
        let split_ms = crate::db::settings_get(&ctx.db, "record.segment_secs")
            .await
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(300)
            * 1000;
        let container = crate::db::settings_get(&ctx.db, "record.format")
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "mkv".into());
        Some(crate::pipeline::RecordOptions {
            enabled: true,
            directory: dir,
            basename,
            container,
            split_ms,
            max_files: 100,
            audio_dev: audio_dev.clone(),
        })
    } else {
        None
    };

    let mut pm = ctx.pipeline.lock().await;
    pm.rebuild(
        &st,
        out_rtmp.as_deref(),
        audio_dev.as_deref(),
        rec_opts,
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({ "ok": true, "enabled": p.enabled })))
}

//----Markers----//

// ----- DTOs -----
#[derive(Deserialize)]
pub struct MarkerCreateBody {
    /// optional, server ignoriert fremde session_id und nimmt aktive Session
    pub text: Option<String>,
    pub marker_type_id: Option<i64>, // default = 1 (Freitext)
                                     // record_time NICHT vom Client; wird serverseitig errechnet
}

#[derive(Deserialize)]
pub struct MarkerPatchBody {
    pub text: Option<Option<String>>, // Some(None) -> auf NULL setzen
    pub preselected: Option<bool>,
}

#[derive(serde::Serialize)]
pub struct MarkersListResponse {
    pub session_id: i64,
    pub markers: Vec<crate::db::MarkerJoined>,
}

#[derive(Deserialize)]
pub struct MarkersQuery {
    #[serde(default)]
    pub with_snapshots: Option<String>,
}

fn marker_snapshot_path(
    rec_dir: &std::path::Path,
    session_id: i64,
    marker_id: i64,
) -> std::path::PathBuf {
    rec_dir
        .join(session_id.to_string())
        .join(format!("marker-{}.jpg", marker_id))
}

fn latest_segment_for_session(base: &str, session_id: i64) -> Option<std::path::PathBuf> {
    // Einzel-Datei: <base>/<session_id>.mkv oder session-<id>.mkv/mp4
    let direct_candidates = [
        std::path::Path::new(base).join(format!("session-{}.mkv", session_id)),
        std::path::Path::new(base).join(format!("session-{}.mp4", session_id)),
        std::path::Path::new(base).join(format!("{}", session_id)),
    ];
    for c in direct_candidates {
        if c.exists() {
            return Some(c);
        }
    }

    // Fallback: Session-Ordner nach mkv/mp4 durchsuchen, jüngste nehmen
    let candidates = [
        std::path::Path::new(base).join(session_id.to_string()),
        std::path::Path::new(base).join(format!("session-{}", session_id)),
    ];

    let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for dir in candidates {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext.eq_ignore_ascii_case("mkv") || ext.eq_ignore_ascii_case("mp4") {
                        if let Ok(md) = entry.metadata() {
                            if let Ok(mtime) = md.modified() {
                                if newest.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                                    newest = Some((mtime, path));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    newest.map(|(_, p)| p)
}

async fn spawn_marker_snapshot(ctx: AppCtx, session_id: i64, marker_id: i64, offset_sec: f64) {
    let rec_dir = crate::db::app_dirs().rec_dir;
    let session_row = crate::db::session_by_id(&ctx.db, session_id)
        .await
        .ok()
        .flatten();
    let filename_from_db = session_row.as_ref().and_then(|r| r.filename.clone());

    // 1) VOD-Datei (wenn vorhanden) bevorzugen
    let input = first_recording_candidate(&rec_dir, session_id, filename_from_db.clone())
        // 2) Danach jüngstes Segment versuchen
        .or_else(|| latest_segment_for_session(&rec_dir.to_string_lossy(), session_id));

    let Some(input) = input else {
        tracing::warn!(session_id, marker_id, "no segment found for snapshot");
        return;
    };

    let out_path = marker_snapshot_path(&rec_dir, session_id, marker_id);
    if let Some(parent) = out_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    // Offset ggf. auf die Videolänge kappen
    let duration_sec = sim::probe_duration_ms(&input).unwrap_or(0) as f64 / 1000.0;
    let mut seek = offset_sec;
    if duration_sec > 0.0 {
        let max_seek = (duration_sec - 0.5).max(0.0);
        if seek > max_seek {
            seek = max_seek;
        }
        if seek < 0.0 {
            seek = 0.0;
        }
    } else if seek < 0.0 {
        seek = 0.0;
    }

    let input_str = input.to_string_lossy().to_string();
    let out_str = out_path.to_string_lossy().to_string();

    tokio::spawn(async move {
        let status = spawn_blocking(move || {
            let mut cmd = Command::new("ffmpeg");
            cmd.arg("-y");
            if seek > 0.0 {
                cmd.arg("-ss").arg(format!("{seek}"));
            }
            cmd.arg("-i").arg(&input_str);
            cmd.arg("-frames:v")
                .arg("1")
                .arg("-q:v")
                .arg("2")
                .arg(&out_str);
            tracing::info!(marker_id, input=%input_str, seek=seek, "creating marker snapshot from");
            cmd.status()
        })
        .await
        .ok()
        .and_then(|r| r.ok());

        if status.map(|s| s.success()).unwrap_or(false) {
            tracing::info!(marker_id, "marker snapshot created");
        } else {
            tracing::warn!(marker_id, "failed to create marker snapshot");
        }
    });
}

async fn ensure_marker_snapshot_if_missing(ctx: AppCtx, session_id: i64, marker_id: i64) {
    let rec_dir = crate::db::app_dirs().rec_dir;
    let path = marker_snapshot_path(&rec_dir, session_id, marker_id);
    if tokio::fs::metadata(&path).await.is_ok() {
        return;
    }
    // record_time in Sekunden (falls vorhanden)
    let offset_sec = crate::db::marker_by_id(&ctx.db, marker_id)
        .await
        .ok()
        .flatten()
        .map(|m| m.record_time as f64)
        .unwrap_or(0.0);
    spawn_marker_snapshot(ctx, session_id, marker_id, offset_sec).await;
}

// ----- POST /api/markers (nur bei aktiver Session) -----
pub async fn markers_create(
    State(ctx): State<AppCtx>,
    headers: HeaderMap,
    Json(body): Json<MarkerCreateBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = auth_from_headers(&ctx, &headers);
    // aktive Session holen
    let Some(session_id) = crate::db::sim_active_session_id(&ctx.db)
        .await
        .map_err(|e| {
            error!(?e, "sim_active_session_id");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
    else {
        return Err(StatusCode::CONFLICT); // keine aktive Session
    };

    let start_time = crate::db::sim_session_start_time(&ctx.db, session_id)
        .await
        .map_err(|e| {
            tracing::error!(?e, "sim_session_start_time");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .unwrap_or(0);

    let now = chrono::Utc::now().timestamp();
    let record_time = (now - start_time).max(0) as i64;

    let marker_type_id = body.marker_type_id.unwrap_or(1);
    let user_id = user.as_ref().map(|u| u.id);
    let text_for_db = body.text.clone();
    let id = crate::db::marker_add(
        &ctx.db,
        session_id,
        marker_type_id,
        user_id,           // user_id
        text_for_db,       // text
        Some(record_time), // record_time
    )
    .await
    .map_err(|e| {
        tracing::error!(?e, "marker_add");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Webhook: Marker gesetzt
    let text_for_hook = body.text.clone();
    let _ = tokio::spawn(trigger_webhook(
        "marker_add",
        json!({ "id": id, "session_id": session_id, "marker_type_id": marker_type_id, "text": text_for_hook }),
    ));

    // UI-Broadcast
    broadcast_ui(
        &ctx,
        json!({
            "type": "marker_add",
            "id": id,
            "session_id": session_id,
            "marker_type_id": marker_type_id,
            "text": body.text,
            "record_time": record_time,
            "user_id": user_id,
            "username": user.as_ref().map(|u| u.username.clone()).unwrap_or_else(|| "not logged in".into()),
        }),
    )
    .await;

    if let Some(u) = user.as_ref() {
        let has_text = body.text.as_ref().map(|t| !t.trim().is_empty()).unwrap_or(false);
        match crate::achievement::on_marker_created(&ctx.db, u.id, record_time, has_text).await {
            Ok(keys) => {
                if !keys.is_empty() {
                    if let Ok(defs) = crate::achievement::achievement_payloads_by_keys(&ctx.db, &keys).await {
                        let msg = json!({
                            "type": "achievement_unlocked",
                            "user_id": u.id,
                            "username": u.username,
                            "achievements": defs,
                        });
                        broadcast_ui(&ctx, msg).await;
                    }
                }
            }
            Err(e) => tracing::warn!(?e, user_id = u.id, "achievement on_marker_created failed"),
        }
    }

    Ok(Json(json!({
        "ok": true,
        "id": id,
        "session_id": session_id,
        "record_time": record_time,
        "user_id": user_id,
        "username": user.as_ref().map(|u| u.username.clone()).unwrap_or_else(|| "not logged in".into())
    })))
}

// ----- GET /api/sessions/:id/markers -----
pub async fn markers_list_for_session(
    State(ctx): State<AppCtx>,
    Path(session_id): Path<i64>,
    Query(q): Query<MarkersQuery>,
) -> Result<Json<MarkersListResponse>, StatusCode> {
    let rows: Vec<_> = crate::db::list_markers_by_session(&ctx.db, session_id)
        .await
        .map_err(|e| {
            tracing::error!(?e, "list_markers_by_session");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .into_iter()
        .map(|mut m| {
            if m.username.is_none() {
                m.username = Some("not logged in".into());
            }
            m
        })
        .collect();

    // Snapshots nur erzeugen, wenn explizit angefordert
    let do_snap = matches!(
        q.with_snapshots.as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    );
    if do_snap {
        let ids: Vec<i64> = rows.iter().map(|m| m.id).collect();
        for mid in ids {
            let ctx_clone = ctx.clone();
            let sid = session_id;
            tokio::spawn(async move {
                ensure_marker_snapshot_if_missing(ctx_clone, sid, mid).await;
            });
        }
    }

    Ok(Json(MarkersListResponse {
        session_id,
        markers: rows,
    }))
}

// ----- GET /api/markers/:id/snapshot -----
pub async fn marker_snapshot(
    State(_ctx): State<AppCtx>,
    Path(marker_id): Path<i64>,
) -> Result<impl IntoResponse, StatusCode> {
    // Wir kennen die Session hier nicht → durchsuche recording-Ordner
    let rec_dir = crate::db::app_dirs().rec_dir;
    let mut found: Option<std::path::PathBuf> = None;
    if let Ok(rd) = std::fs::read_dir(&rec_dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let candidate = path.join(format!("marker-{}.jpg", marker_id));
                if candidate.exists() {
                    found = Some(candidate);
                    break;
                }
            }
        }
    }

    // Fallback: nichts gefunden
    let path = if let Some(p) = found {
        p
    } else {
        return Err(StatusCode::NOT_FOUND);
    };
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    Ok(([(CONTENT_TYPE, "image/jpeg")], bytes))
}

// ----- DELETE /api/markers/:id -----
pub async fn markers_delete(
    State(ctx): State<AppCtx>,
    Path(marker_id): Path<i64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    crate::db::marker_delete(&ctx.db, marker_id)
        .await
        .map_err(|e| {
            error!(?e, "delete marker");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    broadcast_ui(
        &ctx,
        json!({
            "type": "marker_delete",
            "id": marker_id
        }),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

// ----- PATCH /api/markers/:id -----
pub async fn markers_patch(
    State(ctx): State<AppCtx>,
    Path(marker_id): Path<i64>,
    Json(body): Json<MarkerPatchBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let new_text: Option<String> = body.text.clone().flatten();
    let has_text = body.text.is_some();
    if has_text {
        crate::db::update_marker_text(&ctx.db, marker_id, new_text.clone())
            .await
            .map_err(|e| {
                error!(?e, "patch marker text");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    if let Some(preselected) = body.preselected {
        crate::db::update_marker_preselected(&ctx.db, marker_id, preselected)
            .await
            .map_err(|e| {
                error!(?e, "patch marker preselected");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    if has_text || body.preselected.is_some() {
        broadcast_ui(
            &ctx,
            json!({
                "type": "marker_update",
                "id": marker_id,
                "text": new_text,
                "preselected": body.preselected
            }),
        )
        .await;
    }
    Ok(Json(json!({ "ok": true })))
}

// ----- OPTIONAL: Export -----
pub async fn markers_export_json(
    State(ctx): State<AppCtx>,
    Path(session_id): Path<i64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let rows = crate::db::markers_list_joined_by_session(&ctx.db, session_id)
        .await
        .map_err(|e| {
            error!(?e, "export markers");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(json!({ "session_id": session_id, "markers": rows })))
}

// --- Marker-Typen: pro Nutzer + Default ---
#[derive(Serialize)]
pub struct MarkerTypesListResponse {
    pub marker_types: Vec<db::MarkerType>,
}

#[derive(Deserialize)]
pub struct MarkerTypeCreate {
    pub name: String,
    pub color: String,
    pub sort_index: Option<i64>,
    /// Tastenkürzel (optional)
    pub hotkey: Option<String>,
}

#[derive(Deserialize)]
pub struct MarkerTypePatch {
    pub name: Option<String>,
    pub color: Option<String>,
    pub sort_index: Option<i64>,
    /// Tastenkürzel: `None` = unverändert, leerer String = löschen
    pub hotkey: Option<String>,
}

/// Normalisiert ein Tastenkürzel aus dem Request (leer → None).
fn normalize_hotkey(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim).filter(|h| !h.is_empty()).map(|h| h.to_string())
}

/// Prüft, ob `hotkey` bereits von einem anderen für den Nutzer sichtbaren Marker-Typ
/// belegt ist. Liefert den Namen des belegenden Typs.
async fn hotkey_conflict(
    ctx: &AppCtx,
    user_id: Option<i64>,
    hotkey: &str,
    exclude_id: Option<i64>,
) -> Option<String> {
    let all = db::marker_types_list(&ctx.db, user_id).await.ok()?;
    all.into_iter()
        .filter(|t| Some(t.id) != exclude_id)
        .find(|t| t.hotkey.as_deref().map(|h| h.eq_ignore_ascii_case(hotkey)).unwrap_or(false))
        .map(|t| t.name)
}

pub async fn marker_types_list(
    State(ctx): State<AppCtx>,
    headers: HeaderMap,
) -> Result<Json<MarkerTypesListResponse>, StatusCode> {
    let user = auth_from_headers(&ctx, &headers);
    let rows = db::marker_types_list(&ctx.db, user.as_ref().map(|u| u.id))
        .await
        .map_err(|e| {
            tracing::error!(?e, "marker_types_list");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(MarkerTypesListResponse { marker_types: rows }))
}

pub async fn marker_types_create(
    State(ctx): State<AppCtx>,
    headers: HeaderMap,
    Json(body): Json<MarkerTypeCreate>,
) -> Result<Json<db::MarkerType>, StatusCode> {
    let user = auth_from_headers(&ctx, &headers);
    let uid = user.as_ref().map(|u| u.id);
    let sort_index = match body.sort_index {
        Some(v) => Some(v),
        None => {
            if let Some(v) = db::marker_type_default_sort_index(&body.name) {
                Some(v)
            } else {
                db::marker_types_next_sort_index(&ctx.db, uid).await.ok()
            }
        }
    };
    let hotkey = normalize_hotkey(body.hotkey.as_deref());
    if let Some(hk) = hotkey.as_deref() {
        if let Some(other) = hotkey_conflict(&ctx, uid, hk, None).await {
            tracing::warn!(hotkey = hk, other = %other, "marker_types_create: hotkey conflict");
            return Err(StatusCode::CONFLICT);
        }
    }
    let id = db::marker_types_add(&ctx.db, &body.name, &body.color, uid, sort_index)
        .await
        .map_err(|e| {
            tracing::error!(?e, "marker_types_add");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if let Some(hk) = hotkey.as_deref() {
        let _ = db::marker_types_set_hotkey(&ctx.db, id, Some(hk)).await;
    }
    let all = db::marker_types_list(&ctx.db, uid).await.map_err(|e| {
        tracing::error!(?e, "marker_types_list");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let newly = all
        .into_iter()
        .find(|t| t.id == id)
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(newly))
}

pub async fn marker_types_update(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(body): Json<MarkerTypePatch>,
) -> Result<StatusCode, StatusCode> {
    let user = auth_from_headers(&ctx, &headers);
    let old = db::marker_type_get(&ctx.db, id)
        .await
        .map_err(|e| {
            tracing::error!(?e, "marker_type_get");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    let allowed = match user.as_ref() {
        Some(u) if u.role == "admin" => true,
        Some(u) => old.user_id == Some(u.id),
        None => old.user_id.is_none(),
    };
    if !allowed {
        return Err(StatusCode::FORBIDDEN);
    }

    let name = body.name.unwrap_or(old.name);
    let color = body.color.unwrap_or(old.color);
    let sort_index = body.sort_index.or(old.sort_index);
    // Hotkey: None = unverändert, "" = löschen, sonst setzen (mit Konfliktprüfung)
    if let Some(raw) = body.hotkey.as_deref() {
        let hk = normalize_hotkey(Some(raw));
        if let Some(h) = hk.as_deref() {
            let uid = user.as_ref().map(|u| u.id);
            if let Some(other) = hotkey_conflict(&ctx, uid, h, Some(id)).await {
                tracing::warn!(hotkey = h, other = %other, "marker_types_update: hotkey conflict");
                return Err(StatusCode::CONFLICT);
            }
        }
        db::marker_types_set_hotkey(&ctx.db, id, hk.as_deref())
            .await
            .map_err(|e| {
                tracing::error!(?e, "marker_types_set_hotkey");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }
    db::marker_types_update(&ctx.db, id, &name, &color, sort_index)
        .await
        .map_err(|e| {
            tracing::error!(?e, "marker_types_update");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn marker_types_delete(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    let user = auth_from_headers(&ctx, &headers);
    let old = db::marker_type_get(&ctx.db, id)
        .await
        .map_err(|e| {
            tracing::error!(?e, "marker_type_get");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    let allowed = match user.as_ref() {
        Some(u) if u.role == "admin" => true,
        Some(u) => old.user_id == Some(u.id),
        None => old.user_id.is_none(),
    };
    if !allowed {
        return Err(StatusCode::FORBIDDEN);
    }
    db::marker_types_delete(&ctx.db, id).await.map_err(|e| {
        tracing::error!(?e, "marker_types_delete");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(StatusCode::NO_CONTENT)
}

// ===== UI-Content: Ticker & Trivia =====

pub async fn ui_content(State(ctx): State<AppCtx>) -> Result<Json<JsonValue>, StatusCode> {
    let ticker = load_ticker_messages(&ctx.db).await;
    let trivia = load_trivia_items(&ctx.db).await;
    let list: Vec<JsonValue> = trivia
        .into_iter()
        .map(|t| {
            json!({
                "file": t.file,
                "title": t.title,
                "url": format!("/api/trivia/file/{}", t.file),
            })
        })
        .collect();

    Ok(Json(json!({
        "ticker_messages": ticker,
        "trivia_images": list,
        "marker_notes": load_marker_notes(&ctx.db, None).await,
    })))
}

pub async fn ui_nutzerinfo() -> Result<Json<JsonValue>, StatusCode> {
    let candidates = [
        std::path::PathBuf::from("Updates_Snapshot.md"),
        std::path::PathBuf::from("docs/Updates_Snapshot.md"),
        std::path::PathBuf::from("/usr/share/openresquesim/Updates_Snapshot.md"),
    ];
    for p in candidates {
        if let Ok(text) = tokio::fs::read_to_string(&p).await {
            return Ok(Json(json!({
                "ok": true,
                "markdown": text,
                "source": p.to_string_lossy(),
            })));
        }
    }
    Ok(Json(json!({
        "ok": false,
        "markdown": "Kein Updates_Snapshot verfügbar.",
        "source": "",
    })))
}

pub async fn ui_log_mail_to(
    State(ctx): State<AppCtx>,
    _user: AuthUser,
) -> Result<Json<JsonValue>, StatusCode> {
    let mail_to = crate::db::settings_get(&ctx.db, "logs.mail_to")
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    Ok(Json(json!({
        "ok": true,
        "mail_to": mail_to
    })))
}

pub async fn ticker_get(State(ctx): State<AppCtx>) -> Result<Json<JsonValue>, StatusCode> {
    let ticker = load_ticker_messages(&ctx.db).await;
    Ok(Json(json!({ "messages": ticker })))
}

#[derive(Deserialize)]
pub struct TickerPayload {
    pub messages: Vec<String>,
}

pub async fn ticker_set(
    user: AuthUser,
    State(ctx): State<AppCtx>,
    Json(body): Json<TickerPayload>,
) -> Result<Json<JsonValue>, StatusCode> {
    ensure_admin(&user)?;
    save_ticker_messages(&ctx.db, &body.messages).await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn trivia_list(State(ctx): State<AppCtx>) -> Result<Json<JsonValue>, StatusCode> {
    let trivia = load_trivia_items(&ctx.db).await;
    let list: Vec<JsonValue> = trivia
        .into_iter()
        .map(|t| {
            json!({
                "file": t.file,
                "title": t.title,
                "url": format!("/api/trivia/file/{}", t.file),
            })
        })
        .collect();
    Ok(Json(json!({ "items": list })))
}

pub async fn trivia_file(Path(name): Path<String>) -> Result<impl IntoResponse, StatusCode> {
    if name.contains("..") {
        return Err(StatusCode::FORBIDDEN);
    }
    let dir = trivia_dir();
    let path = dir.join(&name);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let ct = if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        match ext.to_ascii_lowercase().as_str() {
            "png" => "image/png",
            "gif" => "image/gif",
            _ => "image/jpeg",
        }
    } else {
        "application/octet-stream"
    };
    Ok(([(CONTENT_TYPE, ct)], bytes))
}

pub async fn trivia_upload(
    user: AuthUser,
    State(ctx): State<AppCtx>,
    mut multipart: Multipart,
) -> Result<Json<JsonValue>, StatusCode> {
    ensure_admin(&user)?;
    let dir = trivia_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut title: Option<String> = None;
    let mut data: Option<Bytes> = None;
    let mut orig_name: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("title") => {
                title = field.text().await.ok();
            }
            Some("file") | None => {
                orig_name = field.file_name().map(|s| s.to_string());
                data = field.bytes().await.ok();
            }
            _ => {}
        }
    }

    let bytes = data.ok_or(StatusCode::BAD_REQUEST)?;
    let ext = orig_name
        .as_deref()
        .and_then(|n| n.rsplit('.').next())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_else(|| "jpg".to_string());
    let ts = chrono::Utc::now().timestamp();
    let fname = format!("trivia_{}.{}", ts, ext);
    let path = dir.join(&fname);
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut list = load_trivia_items(&ctx.db).await;
    list.push(TriviaItem {
        file: fname.clone(),
        title: title.clone(),
    });
    save_trivia_items(&ctx.db, &list).await?;

    Ok(Json(json!({
        "ok": true,
        "file": fname,
        "title": title,
        "url": format!("/api/trivia/file/{}", fname),
    })))
}

pub async fn trivia_delete(
    user: AuthUser,
    State(ctx): State<AppCtx>,
    Path(id): Path<String>,
) -> Result<Json<JsonValue>, StatusCode> {
    ensure_admin(&user)?;
    if id.contains("..") {
        return Err(StatusCode::FORBIDDEN);
    }
    let dir = trivia_dir();
    let path = dir.join(&id);
    let mut list = load_trivia_items(&ctx.db).await;
    let before = list.len();
    list.retain(|t| t.file != id);
    if list.len() == before {
        return Err(StatusCode::NOT_FOUND);
    }
    let _ = tokio::fs::remove_file(&path).await;
    save_trivia_items(&ctx.db, &list).await?;
    Ok(Json(json!({ "ok": true })))
}

// Marker-Notizen (Textbausteine) – offen für alle (keine Auth)
pub async fn marker_notes_get(
    State(ctx): State<AppCtx>,
    headers: HeaderMap,
) -> Result<Json<JsonValue>, StatusCode> {
    let user = auth_from_headers(&ctx, &headers);
    let notes = load_marker_notes(&ctx.db, user.as_ref().map(|u| u.id)).await;
    Ok(Json(json!({ "notes": notes })))
}

#[derive(Deserialize)]
pub struct MarkerNotesPayload {
    pub notes: Vec<String>,
}

pub async fn marker_notes_set(
    State(ctx): State<AppCtx>,
    headers: HeaderMap,
    Json(body): Json<MarkerNotesPayload>,
) -> Result<Json<JsonValue>, StatusCode> {
    let user = auth_from_headers(&ctx, &headers);
    save_marker_notes(&ctx.db, user.as_ref().map(|u| u.id), &body.notes).await?;
    Ok(Json(json!({ "ok": true })))
}

// ============================================================================
// Teilnehmer-Notizblätter
// Ausbilder legen Blätter an (/api/participants, Login), Teilnehmer arbeiten
// per Token-Link ohne Login (/api/tn/:token, Public-Pfad in main.rs).
// ============================================================================

const PARTICIPANT_MARKER_TYPE: &str = "Teilnehmer";
const PARTICIPANT_MARKER_COLOR: &str = "#0ea5e9";

#[derive(Deserialize)]
pub struct ParticipantCreate {
    pub name: String,
}

#[derive(Serialize)]
pub struct ParticipantSheetDto {
    pub id: i64,
    pub token: String,
    pub name: String,
    pub path: String,
    pub session_id: Option<i64>,
    pub session_name: Option<String>,
    pub session_active: bool,
    pub created_at: i64,
    pub updated_at: Option<i64>,
    pub field_count: usize,
}

fn parse_sheet_data(raw: &str) -> serde_json::Map<String, Value> {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

async fn sheet_to_dto(ctx: &AppCtx, s: &db::ParticipantSheet, active_id: Option<i64>) -> ParticipantSheetDto {
    let session_name = match s.session_id {
        Some(sid) => db::sim_session_name(&ctx.db, sid).await.ok().flatten(),
        None => None,
    };
    let data = parse_sheet_data(&s.data);
    ParticipantSheetDto {
        id: s.id,
        token: s.token.clone(),
        name: s.name.clone(),
        path: format!("/tn/{}", s.token),
        session_id: s.session_id,
        session_name,
        session_active: s.session_id.is_some() && s.session_id == active_id,
        created_at: s.created_at,
        updated_at: s.updated_at,
        field_count: data.values().filter(|v| !v.as_str().unwrap_or("").trim().is_empty()).count(),
    }
}

pub async fn participants_list(
    State(ctx): State<AppCtx>,
    _user: AuthUser,
) -> Result<Json<Value>, StatusCode> {
    let rows = db::participant_sheets_list(&ctx.db).await.map_err(|e| {
        error!(?e, "participant_sheets_list");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let active_id = db::sim_active_session_id(&ctx.db).await.ok().flatten();
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(sheet_to_dto(&ctx, r, active_id).await);
    }
    Ok(Json(json!({ "sheets": out })))
}

pub async fn participants_create(
    State(ctx): State<AppCtx>,
    user: AuthUser,
    Json(body): Json<ParticipantCreate>,
) -> Result<Json<ParticipantSheetDto>, StatusCode> {
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let row = db::participant_sheet_create(&ctx.db, name, Some(user.id))
        .await
        .map_err(|e| {
            error!(?e, "participant_sheet_create");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    info!(user = %user.username, sheet_id = row.id, name = %row.name, "participant sheet created");
    let active_id = db::sim_active_session_id(&ctx.db).await.ok().flatten();
    Ok(Json(sheet_to_dto(&ctx, &row, active_id).await))
}

pub async fn participants_delete(
    State(ctx): State<AppCtx>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let ok = db::participant_sheet_delete(&ctx.db, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !ok {
        return Err(StatusCode::NOT_FOUND);
    }
    info!(user = %user.username, sheet_id = id, "participant sheet deleted");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn participants_unlink(
    State(ctx): State<AppCtx>,
    _user: AuthUser,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let ok = db::participant_sheet_set_session(&ctx.db, id, None)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !ok {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- Public (per Token) -----------------------------------------------------

async fn sheet_by_token_or_404(ctx: &AppCtx, token: &str) -> Result<db::ParticipantSheet, StatusCode> {
    if token.len() < 8 || token.len() > 64 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(StatusCode::NOT_FOUND);
    }
    db::participant_sheet_by_token(&ctx.db, token)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)
}

/// GET /api/tn/:token — Blatt + Session-Kontext für die Teilnehmerseite.
pub async fn tn_get(
    State(ctx): State<AppCtx>,
    Path(token): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let sheet = sheet_by_token_or_404(&ctx, &token).await?;
    let active = db::sim_active_session_row(&ctx.db).await.ok().flatten();
    let active_id = active.as_ref().map(|r| r.id);
    let linked_name = match sheet.session_id {
        Some(sid) => db::sim_session_name(&ctx.db, sid).await.ok().flatten(),
        None => None,
    };
    Ok(Json(json!({
        "id": sheet.id,
        "name": sheet.name,
        "data": parse_sheet_data(&sheet.data),
        "updated_at": sheet.updated_at,
        "created_at": sheet.created_at,
        "session_id": sheet.session_id,
        "session_name": linked_name,
        "linked_active": sheet.session_id.is_some() && sheet.session_id == active_id,
        "active_session": active.map(|r| json!({
            "id": r.id, "name": r.name, "start_time": r.start_time
        })),
    })))
}

#[derive(Deserialize)]
pub struct TnFieldChange {
    pub field: String,
    pub label: Option<String>,
    pub value: Option<String>,
}

#[derive(Deserialize)]
pub struct TnSavePayload {
    pub data: Value,
    #[serde(default)]
    pub changes: Vec<TnFieldChange>,
}

/// PUT /api/tn/:token/data — Formulardaten speichern; bei aktiver Verknüpfung
/// entsteht je geändertem Feld ein Marker unter dem Nutzer „Teilnehmer“.
pub async fn tn_save(
    State(ctx): State<AppCtx>,
    Path(token): Path<String>,
    Json(body): Json<TnSavePayload>,
) -> Result<Json<Value>, StatusCode> {
    let sheet = sheet_by_token_or_404(&ctx, &token).await?;
    let data_obj = body.data.as_object().cloned().ok_or(StatusCode::BAD_REQUEST)?;
    // Nur String-Werte, Größe begrenzen
    let mut clean = serde_json::Map::new();
    for (k, v) in data_obj {
        if k.len() > 64 || clean.len() >= 200 {
            continue;
        }
        let sval = match v {
            Value::String(s) => s,
            Value::Null => String::new(),
            other => other.to_string(),
        };
        let sval: String = sval.chars().take(4000).collect();
        clean.insert(k, Value::String(sval));
    }
    let raw = Value::Object(clean).to_string();
    db::participant_sheet_set_data(&ctx.db, sheet.id, &raw)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Marker nur, wenn das Blatt mit der aktuell laufenden Session verknüpft ist
    let mut markers_created = 0usize;
    let active_id = db::sim_active_session_id(&ctx.db).await.ok().flatten();
    if let (Some(sid), true) = (sheet.session_id, sheet.session_id == active_id) {
        if !body.changes.is_empty() {
            let uid = db::ensure_participant_user(&ctx.db).await.ok();
            let type_id = db::ensure_marker_type_named(&ctx.db, PARTICIPANT_MARKER_TYPE, PARTICIPANT_MARKER_COLOR)
                .await
                .ok();
            if let Some(type_id) = type_id {
                for ch in body.changes.iter().take(40) {
                    let value = ch.value.as_deref().unwrap_or("").trim();
                    if value.is_empty() {
                        continue;
                    }
                    let label = ch
                        .label
                        .as_deref()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .unwrap_or(ch.field.as_str());
                    let value: String = value.chars().take(300).collect();
                    let text = format!("{}: {}: {}", sheet.name, label, value);
                    match db::marker_add(&ctx.db, sid, type_id, uid, Some(text.clone()), None).await {
                        Ok(id) => {
                            markers_created += 1;
                            broadcast_ui(
                                &ctx,
                                json!({
                                    "type": "marker_add",
                                    "id": id,
                                    "session_id": sid,
                                    "marker_type_id": type_id,
                                    "text": text,
                                    "user_id": uid,
                                    "username": db::PARTICIPANT_USERNAME,
                                }),
                            )
                            .await;
                        }
                        Err(e) => warn!(?e, "tn_save: marker_add failed"),
                    }
                }
            }
        }
    }
    Ok(Json(json!({ "ok": true, "markers_created": markers_created })))
}

#[derive(Deserialize)]
pub struct TnLinkPayload {
    pub session_id: i64,
}

/// POST /api/tn/:token/link — Blatt mit der laufenden Session verknüpfen.
pub async fn tn_link(
    State(ctx): State<AppCtx>,
    Path(token): Path<String>,
    Json(body): Json<TnLinkPayload>,
) -> Result<Json<Value>, StatusCode> {
    let sheet = sheet_by_token_or_404(&ctx, &token).await?;
    let active_id = db::sim_active_session_id(&ctx.db).await.ok().flatten();
    if active_id != Some(body.session_id) {
        // Session läuft nicht (mehr): nicht verknüpfen
        return Err(StatusCode::CONFLICT);
    }
    db::participant_sheet_set_session(&ctx.db, sheet.id, Some(body.session_id))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = db::ensure_participant_user(&ctx.db).await;
    info!(sheet_id = sheet.id, session_id = body.session_id, "participant sheet linked");
    Ok(Json(json!({ "ok": true, "session_id": body.session_id })))
}

/// POST /api/tn/:token/unlink
pub async fn tn_unlink(
    State(ctx): State<AppCtx>,
    Path(token): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let sheet = sheet_by_token_or_404(&ctx, &token).await?;
    db::participant_sheet_set_session(&ctx.db, sheet.id, None)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "ok": true })))
}

/// POST /api/tn/:token/reset — Formular leeren (Verknüpfung bleibt).
pub async fn tn_reset(
    State(ctx): State<AppCtx>,
    Path(token): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let sheet = sheet_by_token_or_404(&ctx, &token).await?;
    db::participant_sheet_set_data(&ctx.db, sheet.id, "{}")
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    info!(sheet_id = sheet.id, "participant sheet reset");
    Ok(Json(json!({ "ok": true })))
}
