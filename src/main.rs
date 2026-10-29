//! src/main.rs
use crate::web::pager as pager_web;
use crate::web::viewer as viewer_web;
use anyhow::Result;
use axum::extract::{Query, State};
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::{extract::DefaultBodyLimit, http::Method};
use axum::{
    extract::ws::WebSocketUpgrade,
    response::IntoResponse,
    routing::{delete, get, get_service, patch, post},
    Router,
};
use gstreamer as gst;
use sqlx::SqlitePool;
use std::{env, net::SocketAddr, sync::Arc};
use tokio::sync::{broadcast, Mutex};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
//use tower_http::services::ServeDir;
use crate::state::AppCtx;
use gst::prelude::*;
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex as StdMutex;

use crate::pipeline::{build_pipeline, PipelineMetrics, SourceSpec};
//use crate::vod_module;
static PIPELINE_HOLDER: Lazy<Arc<Mutex<Option<gst::Pipeline>>>> =
    Lazy::new(|| Arc::new(Mutex::new(None)));

pub async fn start_pipeline_initial(cameras: Vec<SourceSpec>) {
    let metrics = Arc::new(StdMutex::new(PipelineMetrics::default()));
    let pipeline = build_pipeline(cameras, None, None, 1, None, metrics)
        .expect("pipeline build");
    *PIPELINE_HOLDER.lock().await = Some(pipeline);
}

pub async fn reload_pipeline() {
    if let Some(old) = PIPELINE_HOLDER.lock().await.take() {
        let _ = old.set_state(gst::State::Null);
    }
    let cameras = load_cameras_from_db_or_default();
    let metrics = Arc::new(StdMutex::new(PipelineMetrics::default()));
    let pipeline = build_pipeline(cameras, None, None, 1, None, metrics)
        .expect("pipeline rebuild");
    *PIPELINE_HOLDER.lock().await = Some(pipeline);
}

fn load_cameras_from_db_or_default() -> Vec<SourceSpec> {
    // If you already have a camera table, map rows to SourceSpec { name, uri: Some(url), ... }.
    // Placeholder default (6 slots):
    (1..=6)
        .map(|i| SourceSpec {
            name: Some(format!("camera{}", i)),
            uri: None,     // fill from DB if available
            dev: None,     // v4l2-Gerät (falls vorhanden), sonst None
            is_live: true, // als Live-Quelle behandeln
            pattern: Some((i - 1) as i32),
            is_placeholder: false,
            placeholder_label: None,
        })
        .collect()
}

// re-export for routes.rs
pub mod main_reload {
    pub async fn reload_pipeline() {
        super::reload_pipeline().await
    }
}

// ---- Module deklarieren (damit crate::xyz Pfade aufgelöst werden) ----
mod app_config;
#[path = "Achievement.rs"]
mod achievement;
mod db;
mod gui;
mod host;
mod pipeline;
mod routes;
mod vod;
mod web;
//mod sessions;
//mod simulate;
mod logging;
mod sim;
mod voice;
mod state;
mod stream_auth;
mod pairing;
mod https_admin;
mod privileged;
mod web_frontend;
mod ws;

// helperscripts/kamera.rs liegt in einem Unterordner
mod helperscripts {
    pub mod kamera;
    pub mod preview_helper;
    pub mod ptz;
}
fn is_public_api_path(path: &str) -> bool {
    if matches!(
        path,
        "/api/qr" | "/api/setup/status" | "/api/setup/complete" | "/api/host/ip"
    ) {
        return true;
    }
    if matches!(path, "/api/ui/content" | "/api/ticker" | "/api/trivia") {
        return true;
    }
    // On-Air-Relais & ähnliche IoT-Geräte ohne Token-Handling: reiner Lesezugriff,
    // keine sensiblen Daten (nur ob/welche Übung aktuell läuft).
    if path == "/api/sim/status" {
        return true;
    }
    // Kopplung abschließen: vom ungekoppelten Gerät aus, geschützt durch PIN/QR-Code
    if path == "/api/pairing/claim" {
        return true;
    }
    if path.starts_with("/api/trivia/file/") {
        return true;
    }
    // Teilnehmer-Notizblätter: Zugriff per geheimem Token-Link ohne Login
    if path.starts_with("/api/tn/") {
        return true;
    }
    if path.starts_with("/api/sessions/") && path.ends_with("/snapshot") {
        return true;
    }
    if path.starts_with("/api/markers/") && path.ends_with("/snapshot") {
        return true;
    }
    if path.starts_with("/api/viewers/") && path.ends_with("/ws") {
        return true;
    }
    if path.starts_with("/api/viewers/") && path.ends_with("/report") {
        return true;
    }
    false
}

async fn require_api_login(
    State(ctx): State<AppCtx>,
    req: Request<axum::body::Body>,
    next: Next<axum::body::Body>,
) -> Result<Response, StatusCode> {
    let path = req.uri().path();
    let method = req.method();
    let is_mutating_api = path.starts_with("/api/")
        && !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    let auth_user = crate::routes::auth_from_headers(&ctx, req.headers());

    if is_mutating_api {
        if let Some(u) = &auth_user {
            tracing::info!(
                user_id = u.id,
                username = %u.username,
                role = %u.role,
                method = %method,
                path = %path,
                "api action"
            );
        } else {
            tracing::warn!(method = %method, path = %path, "api action without valid auth token");
        }
    }

    if !path.starts_with("/api/") {
        return Ok(next.run(req).await);
    }
    if method == Method::OPTIONS || is_public_api_path(path) {
        return Ok(next.run(req).await);
    }
    if auth_user.is_none() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(req).await)
}

#[derive(Default)]
struct SetupArgs {
    admin_password: Option<String>,
    data_dir: Option<String>,
    db_path: Option<String>,
    marker_types: Vec<(String, String)>,
    no_default_markers: bool,
}

fn parse_setup_args(args: &[String]) -> anyhow::Result<SetupArgs> {
    let mut out = SetupArgs::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let next = || -> anyhow::Result<String> {
            if i + 1 >= args.len() {
                anyhow::bail!("missing value for {arg}");
            }
            Ok(args[i + 1].clone())
        };
        if let Some(v) = arg.strip_prefix("--admin-password=") {
            out.admin_password = Some(v.to_string());
            i += 1;
            continue;
        }
        if let Some(v) = arg.strip_prefix("--data-dir=") {
            out.data_dir = Some(v.to_string());
            i += 1;
            continue;
        }
        if let Some(v) = arg.strip_prefix("--db-path=") {
            out.db_path = Some(v.to_string());
            i += 1;
            continue;
        }
        if let Some(v) = arg.strip_prefix("--marker-type=") {
            let (name, color) = parse_marker_type(v, None)?;
            out.marker_types.push((name, color));
            i += 1;
            continue;
        }
        match arg.as_str() {
            "--admin-password" | "-p" => {
                out.admin_password = Some(next()?);
                i += 2;
            }
            "--data-dir" => {
                out.data_dir = Some(next()?);
                i += 2;
            }
            "--db-path" => {
                out.db_path = Some(next()?);
                i += 2;
            }
            "--marker-type" => {
                let val = next()?;
                let follow = args.get(i + 2).cloned();
                let (name, color) = parse_marker_type(&val, follow.as_deref())?;
                out.marker_types.push((name, color));
                i += if val.contains(':') || val.contains(',') { 2 } else { 3 };
            }
            "--no-default-markers" => {
                out.no_default_markers = true;
                i += 1;
            }
            _ => {
                anyhow::bail!("unknown setup arg: {arg}");
            }
        }
    }
    Ok(out)
}

fn parse_marker_type(val: &str, next: Option<&str>) -> anyhow::Result<(String, String)> {
    if let Some((name, color)) = val.split_once(':').or_else(|| val.split_once(',')) {
        return Ok((name.trim().to_string(), color.trim().to_string()));
    }
    let Some(color) = next else {
        anyhow::bail!("marker type requires name and color (e.g. \"Label\" \"#ff0000\")");
    };
    Ok((val.trim().to_string(), color.trim().to_string()))
}

fn default_marker_types() -> Vec<(String, String)> {
    vec![
        ("Beginn".to_string(), "#2ecc71".to_string()),
        ("Einsatz".to_string(), "#f1c40f".to_string()),
        ("Hinweis".to_string(), "#3498db".to_string()),
        ("Problem".to_string(), "#e74c3c".to_string()),
    ]
}

async fn run_setup(args: SetupArgs) -> anyhow::Result<()> {
    if let Some(dir) = args.data_dir.as_deref() {
        env::set_var("ORS_DATA_DIR", dir);
    }
    if let Some(path) = args.db_path.as_deref() {
        env::set_var("ORS_DB_PATH", path);
    }

    let admin_password = args
        .admin_password
        .or_else(|| env::var("ORS_DEFAULT_ADMIN_PASSWORD").ok())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("admin password missing: use --admin-password or ORS_DEFAULT_ADMIN_PASSWORD"))?;

    let pool: SqlitePool = db::init_db_default().await?;
    sqlx::migrate!().run(&pool).await.expect("run migrations");

    let admin_hash = crate::routes::hash_password(&admin_password)?;
    db::ensure_admin_user(&pool, "admin", &admin_hash).await?;

    let mut desired = Vec::new();
    if !args.no_default_markers {
        desired.extend(default_marker_types());
    }
    desired.extend(args.marker_types);

    let existing = db::marker_types_list(&pool, None).await?;
    let mut existing_names: HashSet<String> = existing.into_iter().map(|t| t.name).collect();

    let mut inserted = 0;
    for (name, color) in desired {
        if existing_names.contains(&name) {
            continue;
        }
        let next = db::marker_types_next_sort_index(&pool, None).await.unwrap_or(1);
        let sort_index = db::marker_type_default_sort_index(&name).or(Some(next));
        let _ = db::marker_types_add(&pool, &name, &color, None, sort_index).await?;
        existing_names.insert(name);
        inserted += 1;
    }

    println!("setup ok: admin user updated, marker types added: {inserted}");
    Ok(())
}

fn build_api_router() -> Router<AppCtx> {
    // Verzeichnis für VOD-Dateien bestimmen
    let dirs = db::app_dirs();
    let vod_dir = dirs.rec_dir.join("vod");
    // DASH-Verzeichnis (optional)
    // Statischer File-Service für /vod
    let vod_service = get_service(ServeDir::new(vod_dir));

    Router::new()
        // WebSocket
        .route(
            "/",
            get(|| async { axum::response::Html(include_str!("web_index_fallback.html")) }),
        )
        .route("/ws", get(ws::ws_route))
        // Auth
        .route("/auth/login", post(routes::auth_login))
        .route("/auth/register", post(routes::auth_register))
        .route("/auth/me", get(routes::auth_me))
        // Setup Wizard
        .route("/api/setup/status", get(routes::setup_status))
        .route("/api/setup/complete", post(routes::setup_complete))
        .route(
            "/api/admin/settings",
            get(routes::admin_settings_get).post(routes::admin_settings_set),
        )
        .route(
            "/api/users",
            get(routes::users_list_handler).post(routes::users_create_handler),
        )
        .route("/api/users/:id", delete(routes::users_delete_handler))
        .route("/api/users/:id/stats", get(routes::user_stats_handler))
        .route("/api/users/:id/achievements", get(routes::user_achievements_handler))
        .route("/api/admin/users/stats", get(routes::admin_user_stats_handler))
        .route("/api/ui/nutzerinfo", get(routes::ui_nutzerinfo))
        .route("/api/ui/log_mail_to", get(routes::ui_log_mail_to))
        .route("/api/qr", get(routes::qr_code_svg))
        .route("/api/admin/invite_tokens", post(routes::admin_invite_token_create))
        // Settings & Layouts & Sources
        .route("/api/set_source", post(routes::set_source))
        .route("/api/set_layout", post(routes::set_layout))
        .route("/api/layouts", get(routes::layouts_list_handler))
        .route("/api/rebuild", post(routes::rebuild_handler))
        .route("/api/settings/layout", post(routes::set_layout))
        .route("/ws/webrtc", get(ws_webrtc_route))
        .route("/ws/logs", get(logs_ws_route))
        .route("/api/health", get(routes::health))
        .route("/api/options/vlc", post(routes::vlc_toggle))
        // Voice of Room (PTT → Raumlautsprecher/Viewer)
        .route("/api/voice/status", get(voice::voice_status))
        .route("/api/voice/speakers", get(voice::speakers_list))
        .route("/api/voice/groups", get(voice::groups_list))
        .route(
            "/api/voice/speakers/:id",
            patch(voice::speaker_patch).delete(voice::speaker_delete),
        )
        .route("/api/voice/speakers/:id/cmd", post(voice::speaker_cmd))
        .route("/ws/voice/tx", get(voice::tx_ws))
        .route("/ws/voice/viewer", get(voice::viewer_ws))
        .route("/ws/voice/speaker", get(voice::speaker_ws))
        .route("/api/voice/speakers/:id/approve", post(voice::speaker_approve))
        .route("/api/voice/speakers/:id/revoke", post(voice::speaker_revoke))
        // Alte Pfade (Kompatibilität)
        .route("/ws/paging", get(voice::tx_ws))
        .route("/ws/paging/viewer", get(voice::viewer_ws))
        // Host
        .route("/api/host/stats", get(host::stats_handler))
        .route("/api/host/ip", get(host::ip_handler))
        .route("/api/host/reboot", post(host::reboot_handler))
        .route("/api/host/poweroff", post(host::poweroff_handler))
        .route("/api/host/service/restart", post(host::restart_service_handler))
        .route("/api/host/audio_inputs", get(host::audio_inputs_handler))
        .route("/api/host/logs", get(host::logs_handler))
        .route("/api/admin/logs/files", get(host::logs_files_handler))
        .route(
            "/api/admin/logs/files/:name",
            get(host::logs_file_download_handler),
        )
        .route("/api/admin/logs/cleanup", post(host::logs_cleanup_handler))
        .route("/api/audio/sources/list", get(routes::audio_sources_list))
        .route("/api/audio/state", get(routes::audio_state))
        .route("/api/audio/volume/set", post(routes::audio_volume_set))
        .route("/api/audio/mute/toggle", post(routes::audio_mute_toggle))
        // Audio-Inputs (Definitions)
        .route(
            "/api/audio/inputs",
            get(routes::audio_inputs_defs_get).post(routes::audio_inputs_defs_set),
        )
        // Admin Update (Upload/Install)
        .route("/api/admin/update/list", get(routes::admin_update_list))
        .route(
            "/api/admin/update/upload",
            post(routes::admin_update_upload).layer(DefaultBodyLimit::max(200 * 1024 * 1024)),
        )
        .route(
            "/api/admin/update/install",
            post(routes::admin_update_install),
        )
        .route("/api/admin/update/status", get(routes::admin_update_status))
        // Simulate
        .route("/api/sim/prestart", post(sim::sim_prestart))
        .route("/api/sim/commit", post(sim::sim_commit))
        .route("/api/sim/start", post(sim::sim_start))
        .route("/api/sim/stop", post(sim::sim_stop))
        .route("/api/sim/switch_room", post(sim::sim_switch_room))
        .route("/api/sim/status", get(sim::sim_status))
        .route("/api/sim/debrief/start", post(sim::debrief_start))
        .route("/api/sim/debrief/position", post(sim::debrief_set_position))
        .route("/api/sim/debrief/position", get(sim::debrief_get_position))
        .route("/api/sim/last_finished", get(sim::last_finished))
        .route("/api/sim/stop_all", post(sim::sim_stop_all))
        // Teilnehmer-Notizblätter (Ausbilder: Login; Teilnehmer: Token)
        .route(
            "/api/participants",
            get(routes::participants_list).post(routes::participants_create),
        )
        .route("/api/participants/:id", delete(routes::participants_delete))
        .route("/api/participants/:id/unlink", post(routes::participants_unlink))
        .route("/api/tn/:token", get(routes::tn_get))
        .route("/api/tn/:token/data", axum::routing::put(routes::tn_save))
        .route("/api/tn/:token/link", post(routes::tn_link))
        .route("/api/tn/:token/unlink", post(routes::tn_unlink))
        .route("/api/tn/:token/reset", post(routes::tn_reset))
        // Sessions mgmt
        .route(
            "/api/sessions",
            get(routes::list_sessions).delete(routes::delete_all_sessions),
        )
        .route("/api/sessions/:id", delete(routes::delete_one_session))
        .route("/api/sessions/:id/snapshot", get(routes::session_snapshot))
        // Back-compat für alten Typo
        .route("/api/sim/delete_all", post(routes::delete_all_sessions))
        // Marker
        .route("/api/markers", post(routes::markers_create))
        .route(
            "/api/markers/:id",
            delete(routes::markers_delete).patch(routes::markers_patch),
        )
        .route(
            "/api/sessions/:id/markers",
            get(routes::markers_list_for_session),
        )
        .route("/api/markers/:id/snapshot", get(routes::marker_snapshot))
        // Marker-Typen
        .route(
            "/api/marker_types",
            get(routes::marker_types_list).post(routes::marker_types_create),
        )
        .route(
            "/api/marker_types/:id",
            patch(routes::marker_types_update).delete(routes::marker_types_delete),
        )
        // UI Content (Ticker/Trivia)
        .route("/api/ui/content", get(routes::ui_content))
        .route(
            "/api/ticker",
            get(routes::ticker_get).post(routes::ticker_set),
        )
        .route(
            "/api/trivia",
            get(routes::trivia_list).post(routes::trivia_upload),
        )
        .route("/api/trivia/:id", delete(routes::trivia_delete))
        .route("/api/trivia/file/:name", get(routes::trivia_file))
        // Marker-Textbausteine
        .route(
            "/api/marker_notes",
            get(routes::marker_notes_get).post(routes::marker_notes_set),
        )
        .route(
            "/api/sessions/:id/markers/export.json",
            get(routes::markers_export_json),
        )
        // Viewers
        .route(
            "/api/viewers",
            get(routes::viewers_list).post(routes::viewers_create),
        )
        .route(
            "/api/viewers/:id",
            patch(routes::viewers_patch).delete(routes::viewers_delete),
        )
        .route("/api/viewers/:id/ws", get(routes::viewers_ws))
        // HTTPS einrichten (Admin) und Wurzelzertifikat herunterladen (öffentlich)
        .route("/api/admin/https/status", get(https_admin::https_status))
        .route("/api/admin/https/setup", post(https_admin::https_setup))
        .route("/ca/ors-rootCA.crt", get(https_admin::root_ca_download))
        // Lizenztexte (eingebettet, damit sie auch im Paket verfügbar sind)
        .route("/licenses/gpl", get(|| async { ([("Content-Type", "text/plain; charset=utf-8")], include_str!("../LICENSE")) }))
        .route("/licenses/third-party", get(|| async { ([("Content-Type", "text/plain; charset=utf-8")], include_str!("../THIRD_PARTY_NOTICES.md")) }))
        .route("/api/pairing/start", post(pairing::pairing_start))
        .route("/api/pairing/claim", post(pairing::pairing_claim))
        .route("/api/pairings", get(pairing::pairings_list))
        .route("/api/pairings/:id", delete(pairing::pairing_delete))
        .route("/api/viewers/:id/cmd", post(routes::viewers_cmd))
        .route("/api/viewers/:id/report", post(routes::viewers_report))
        .route("/api/viewers/:id/report_ack", post(routes::viewers_report_ack))
        // Viewer-Frontend
        .route("/viewer/bg_low_poly_alone.png", get(viewer_web::viewer_bg))
        .route("/viewer/favicon.ico", get(viewer_web::viewer_favicon))
        .route("/viewer/:id", get(viewer_web::viewer_page))
        .route("/viewer/viewer.js", get(viewer_web::viewer_js))
        .route("/viewer/viewer.css", get(viewer_web::viewer_css))
        .route("/pager/:id", get(pager_web::pager_page))
        .route("/pager/pager.js", get(pager_web::pager_js))
        .route("/pager/pager.css", get(pager_web::pager_css))
        // Optionen
        .route("/api/camera/brands", get(routes::camera_brands))
        // Kamera-DB
        .route(
            "/api/cameras",
            get(routes::cameras_list_handler).post(routes::cameras_create_handler),
        )
        .route(
            "/api/cameras/:id",
            patch(routes::cameras_update_handler).delete(routes::cameras_delete_handler),
        )
        .route(
            "/api/cameras/:id/snapshot",
            get(routes::camera_snapshot).post(routes::camera_snapshot_force),
        )
        // PTZ (Pan/Tilt/Zoom): jeder eingeloggte User darf steuern, Einrichten bleibt admin-only
        .route("/api/cameras/:id/ptz", post(routes::camera_ptz_move))
        // Legacy/Alias für Frontends, die /api/camera_preview/<id> nutzen
        .route("/api/camera_preview/:id", get(routes::camera_snapshot))
        // einfacher Helfer bleibt separat
        .route("/api/camera/add_simple", post(routes::camera_add_simple))
        // VOD-API
        .nest_service("/vod", vod_service)
        // HLS-Output
        .route("/api/sessions/:id/vod", get(vod::vod_list_for_session))
        // Rooms
        .route(
            "/api/rooms",
            get(routes::rooms_list).post(routes::rooms_create),
        )
        .route(
            "/api/rooms/:id",
            get(routes::rooms_get)
                .patch(routes::rooms_patch)
                .delete(routes::rooms_delete),
        )

    // Statisches Webpanel (dein src/web)
}

// ---- GUI muss im Main-Thread laufen ----
fn gui_entrypoint(server_base: String) -> Result<()> {
    if let Err(e) = gui::run_gui(server_base) {
        eprintln!("GUI error: {e}");
    }
    Ok(())
}

/// Schlüssel zum Signieren der Anmelde-Tokens (JWT). Reihenfolge: ORS_AUTH_SECRET, dann
/// das in der DB gespeicherte (Einrichtungsassistent). Fehlt beides – ältere, nur
/// aktualisierte Installationen oder Einrichtung ohne Assistent –, wird einmalig ein
/// zufälliger Schlüssel erzeugt und gespeichert. Früher galt dann der öffentlich bekannte
/// Standardwert "change-me-please", mit dem sich jeder gültige Tokens ausstellen konnte.
/// Folge beim ersten Start nach dem Update: bestehende Anmeldungen werden ungültig,
/// alle melden sich einmal neu an.
async fn load_or_create_auth_secret(pool: &SqlitePool) -> anyhow::Result<String> {
    const INSECURE_DEFAULT: &str = "change-me-please";
    if let Ok(v) = env::var("ORS_AUTH_SECRET") {
        let v = v.trim().to_string();
        if !v.is_empty() && v != INSECURE_DEFAULT {
            return Ok(v);
        }
    }
    if let Ok(Some(v)) = db::settings_get(pool, "auth.secret").await {
        let v = v.trim().to_string();
        if !v.is_empty() && v != INSECURE_DEFAULT {
            return Ok(v);
        }
    }
    use rand_core::RngCore;
    let mut buf = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut buf);
    let secret: String = buf.iter().map(|b| format!("{b:02x}")).collect();
    db::settings_set(pool, "auth.secret", &secret).await?;
    tracing::warn!(
        "AUTH: kein Schlüssel gesetzt – zufälligen Schlüssel erzeugt und gespeichert. \
         Bestehende Anmeldungen sind ungültig, bitte einmal neu anmelden."
    );
    Ok(secret)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    crate::logging::set_tokio_handle(tokio::runtime::Handle::current());

    let args: Vec<String> = env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("setup")
        || args.get(1).map(|s| s.as_str()) == Some("--setup")
    {
        let setup_args = parse_setup_args(&args[2..])?;
        return run_setup(setup_args).await;
    }

    let log_path = logging::init_tracing_with_file();
    tracing::info!(
        "OpenRescueSim Server v{} startet …",
        env!("CARGO_PKG_VERSION")
    );
    tracing::info!("Logdatei: {}", log_path.display());

    std::thread::spawn(|| {
        let ctx = glib::MainContext::default();
        // kein "ctx.push_thread_default()" nötig, wir nutzen die default
        let loop_ = glib::MainLoop::new(Some(&ctx), false);
        tracing::info!("GLib mainloop started");
        loop_.run(); // blockt in diesem Thread
    });

    // Bind-Adresse (ENV: ORS_BIND, default 0.0.0.0:8393)
    let bind_host = env::var("ORS_BIND").unwrap_or_else(|_| "0.0.0.0:8393".to_string());
    let addr: SocketAddr = bind_host.parse()?;

    // Datenverzeichnis und DB-Pfad

    let pool: SqlitePool = db::init_db_default().await?;
    sqlx::migrate!().run(&pool).await.expect("run migrations");
    pairing::init_db(&pool).await?;
    let auth_secret = Arc::new(load_or_create_auth_secret(&pool).await?);
    let setup_done = db::settings_get(&pool, "setup.done")
        .await
        .ok()
        .flatten()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "y" | "on"))
        .unwrap_or(true);
    if setup_done {
        if let Ok(default_admin_pw) = env::var("ORS_DEFAULT_ADMIN_PASSWORD") {
            if !default_admin_pw.trim().is_empty() {
                let admin_hash = crate::routes::hash_password(&default_admin_pw)?;
                db::ensure_admin_user(&pool, "admin", &admin_hash).await?;
            }
        }
    }
    // Globaler Status + Broadcast
    let status = state::new_status();
    let (tx, _rx) = broadcast::channel::<state::Status>(64);
    let (webrtc_shutdown, _webrtc_rx) = broadcast::channel::<()>(16);

    // Pipeline
    let pm = Arc::new(Mutex::new(pipeline::PipelineManager::new()));
    let ui_broadcast: Arc<Mutex<Vec<tokio::sync::mpsc::UnboundedSender<String>>>> =
        Arc::new(Mutex::new(Vec::new()));

    // AppCtx
    let ctx = state::AppCtx {
        db: pool.clone(),
        status: status.clone(),
        tx: tx.clone(),
        pipeline: pm.clone(),
        viewer_channels: Arc::new(Mutex::new(HashMap::new())),
        ui_broadcast: ui_broadcast.clone(),
        auth_secret: auth_secret.clone(),
        webrtc_shutdown,
    };
    crate::logging::set_ui_broadcast(ctx.ui_broadcast.clone());
    // Ein Vor-Start überlebt keinen Neustart (seine Pipeline ist weg) – Merker aufräumen
    sim::cancel_pending_prestart(&ctx, "Serverstart").await;

    let pool_clone = pool.clone();
    tokio::spawn(async move {
        if let Err(e) = vod::run_transcode_worker(pool_clone).await {
            tracing::error!("VOD worker error: {e}");
        }
    });

    // API als Router<AppCtx> (mit zentraler Login-Pflicht für /api/*)
    let api = build_api_router()
        .route_layer(middleware::from_fn_with_state(
            ctx.clone(),
            require_api_login,
        ))
        .with_state(ctx.clone());

    // UI als Router<AppCtx>
    let ui = web_frontend::mount_web(axum::Router::new()).with_state(ctx.clone());

    // CORS (für UI-Aufrufe aus anderen Origins, z.B. 127.0.0.1 vs. Hostname)
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ]);

    // beide (jetzt beides Router<AppCtx>) mergen
    // Größeres Body-Limit (z.B. für .deb Uploads) auf 200 MB setzen
    let app = ui
        .merge(api)
        .layer(cors)
        .layer(DefaultBodyLimit::max(200 * 1024 * 1024));

    // Optionaler HTTPS-Listener (für Mikrofonzugriff im Browser: PTT auf iPad/PC).
    // ENV: ORS_TLS_CERT + ORS_TLS_KEY (PEM), ORS_TLS_BIND (Default 0.0.0.0:8443).
    // HTTP auf ORS_BIND bleibt parallel aktiv (Viewer, Pager, Lautsprecher, Notizblätter).
    if let Some((cert, key)) = env::var("ORS_TLS_CERT").ok().zip(env::var("ORS_TLS_KEY").ok()) {
        let tls_bind = env::var("ORS_TLS_BIND").unwrap_or_else(|_| "0.0.0.0:8443".to_string());
        match (tls_bind.parse::<SocketAddr>(), std::path::Path::new(&cert).is_file(), std::path::Path::new(&key).is_file()) {
            (Ok(tls_addr), true, true) => {
                match axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key).await {
                    Ok(tls_config) => {
                        let tls_app = app.clone();
                        println!("Listening on https://{tls_addr} (cert: {cert})");
                        tokio::spawn(async move {
                            if let Err(e) = axum_server::bind_rustls(tls_addr, tls_config)
                                .serve(tls_app.into_make_service_with_connect_info::<SocketAddr>())
                                .await
                            {
                                tracing::error!(?e, "HTTPS listener beendet");
                            }
                        });
                    }
                    Err(e) => tracing::error!(?e, cert, key, "TLS-Zertifikat/Key nicht ladbar, HTTPS deaktiviert"),
                }
            }
            (Err(e), _, _) => tracing::error!(?e, bind = %tls_bind, "ORS_TLS_BIND ungültig, HTTPS deaktiviert"),
            _ => tracing::error!(cert, key, "ORS_TLS_CERT/ORS_TLS_KEY zeigen auf keine Datei, HTTPS deaktiviert"),
        }
    } else {
        tracing::info!("HTTPS deaktiviert (ORS_TLS_CERT/ORS_TLS_KEY nicht gesetzt); Mikrofon/PTT braucht HTTPS oder localhost");
    }

    println!("Listening on http://{addr}");
    axum::Server::bind(&addr)
        // ConnectInfo: Lautsprecher-Registrierung braucht die Quell-IP
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await?;

    // GUI im Haupt-Thread (nutzt lokalen Server)
    let gui_host = env::var("ORS_GUI_BASE").unwrap_or_else(|_| "http://127.0.0.1:8393".to_string());
    gui_entrypoint(gui_host)?;

    Ok(())
}

async fn ws_webrtc_route(
    ws: WebSocketUpgrade,
    Query(q): Query<stream_auth::WsAuthQuery>,
    State(ctx): State<AppCtx>,
) -> Response {
    // Live-Bild nur eingeloggt oder mit Video-Ticket der laufenden Session
    if !stream_auth::may_watch_video(&ctx, &q).await {
        tracing::warn!("ws/webrtc: Verbindung ohne gültigen Nachweis abgelehnt");
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // ctx.pipeline ist: Arc<tokio::sync::Mutex<PipelineManager>>
    let pm = ctx.pipeline.clone();
    let shutdown = ctx.webrtc_shutdown.subscribe();
    ws.on_upgrade(move |socket| async move {
        crate::ws::handle_webrtc_ws(socket, pm, shutdown).await;
    })
    .into_response()
}

async fn logs_ws_route(
    ws: WebSocketUpgrade,
    Query(q): Query<stream_auth::WsAuthQuery>,
    State(ctx): State<AppCtx>,
) -> Response {
    // Live-Logs nur für Admins (enthalten IPs, Namen, Konfiguration)
    match stream_auth::user_from_query(&ctx, &q) {
        Some(u) if u.role == "admin" => {}
        Some(u) => {
            tracing::warn!(user = %u.username, role = %u.role, "Live-Logs abgelehnt: kein Admin");
            return StatusCode::FORBIDDEN.into_response();
        }
        None => {
            tracing::warn!(token = q.token.is_some(), "Live-Logs abgelehnt: Token fehlt oder ungültig");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    let log_path = crate::logging::log_file_path();
    ws.on_upgrade(move |socket| async move {
        crate::logging::stream_logs_ws(socket, log_path).await;
    })
    .into_response()
}
