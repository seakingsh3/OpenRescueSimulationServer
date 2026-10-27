// src/sim.rs
use crate::{db, routes::broadcast_ui, state::AppCtx};
use axum::http::{HeaderMap, StatusCode};
use axum::{extract::State, Json};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use std::{fs, path::PathBuf};
use gstreamer as gst;
use gstreamer::prelude::ElementExtManual;

use crate::pipeline::SlotGeom;

async fn wait_for_pipeline_playing(ctx: &AppCtx, timeout: Duration) -> bool {
    let pipeline = {
        let pm = ctx.pipeline.lock().await;
        pm.pipeline.clone()
    };
    let Some(p) = pipeline else {
        return false;
    };
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if p.current_state() == gst::State::Playing {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn stop_pipeline_soft(ctx: &AppCtx, label: &str) {
    tracing::info!("sim_stop: pipeline stop requested ({})", label);
    let _ = ctx.webrtc_shutdown.send(());
    tracing::info!("sim_stop: webrtc shutdown broadcast ({})", label);
    let lock = tokio::time::timeout(Duration::from_secs(2), ctx.pipeline.lock()).await;
    match lock {
        Ok(mut pm) => {
            tracing::info!("sim_stop: pipeline mutex acquired ({})", label);
            let res = pm.hard_reset();
            match res {
                Ok(()) => tracing::info!("sim_stop: pipeline hard reset done ({})", label),
                Err(e) => tracing::warn!("sim_stop: pipeline hard reset failed ({label}): {e:?}"),
            }
        }
        Err(_) => tracing::warn!("sim_stop: pipeline stop timed out ({label}), mutex busy"),
    }
    schedule_memory_trim();
}

/// Gibt nach dem Abbau einer Pipeline freien Heap-Speicher ans System zurück.
/// glibc behält freigegebene Blöcke (v.a. 1080p-Videopuffer aus vielen Threads) sonst
/// in seinen Arenen: gemessen wuchs der RSS über 10 Sessions mit Zuschauer auf ~600 MB,
/// davon waren nach malloc_trim nur noch ~160 MB belegt. Kurz verzögert, damit die
/// asynchron laufenden Aufräumarbeiten der Zuschauer-Zweige vorher fertig sind.
fn schedule_memory_trim() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let _ = tokio::task::spawn_blocking(|| unsafe { libc::malloc_trim(0) }).await;
    });
}

/// Start einer neuen Simulation/Session
#[derive(Deserialize)]
pub struct SimStartPayload {
    pub name: Option<String>,
    pub viewer_ids: Option<Vec<i64>>, // mehrfach möglich
    #[serde(default)]
    pub viewer_id: Option<i64>, // Backward-Compat: einzelner Viewer
    pub viewer_map: Option<Vec<ViewerSelection>>, // neue, pro-Viewer Einstellungen
    pub room_id: Option<i64>,         // NEU – Raum-Placeholder
}

fn dash_url(session_id: i64) -> String {
    let base = std::env::var("DASH_PUBLIC_BASE").unwrap_or_else(|_| "".into()); // optional: z.B. https://example/dash
    if base.is_empty() {
        format!("/dash/{}/manifest.mpd", session_id)
    } else {
        format!(
            "{}/dash/{}/manifest.mpd",
            base.trim_end_matches('/'),
            session_id
        )
    }
}

fn viewer_role_from_id(id: i64) -> &'static str {
    if (1..=49).contains(&id) {
        "instructor"
    } else if (50..=99).contains(&id) {
        "participant"
    } else if id >= 100 {
        "other"
    } else {
        "participant"
    }
}

/// Vor-Start: bereitet Pipeline vor, ohne Viewer/Recording zu triggern
#[derive(Deserialize)]
pub struct SimPrestartPayload {
    pub name: Option<String>,
    pub instructor_ids: Option<Vec<i64>>, // optionale Ausbilder-Benachrichtigung
    pub viewer_map: Option<Vec<ViewerSelection>>,
    pub room_id: Option<i64>,
}

/// Final-Start nach Vor-Start
#[derive(Deserialize)]
pub struct SimCommitPayload {
    pub viewer_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub viewer_id: Option<i64>,
    pub viewer_map: Option<Vec<ViewerSelection>>,
    pub room_id: Option<i64>,
    pub name: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct ViewerSelection {
    pub id: i64,
    pub mode: Option<String>, // "webrtc" | "dash"
    pub role: Option<String>, // "instructor" | "participant"
}

#[derive(Serialize)]
pub struct SimStartResponse {
    pub session_id: i64,
    pub name: String,
    pub recording_folder: String,
    pub record_is_active: bool,
    pub start_time: i64,
}

/// ------------------------------------------------------------
/// POST /api/sim/commit  (finaler Start nach Vor-Start)
/// ------------------------------------------------------------
pub async fn sim_commit(
    State(ctx): State<AppCtx>,
    headers: HeaderMap,
    Json(payload): Json<SimCommitPayload>,
) -> Result<Json<SimStartResponse>, (StatusCode, Json<serde_json::Value>)> {
    let mut created_rec_dir: Option<PathBuf> = None;
    let user_id = crate::achievement::auth_user_id_from_headers(&ctx, &headers);

    // aktive Session blocken
    if let Ok(Some(_sid)) = db::sim_active_session_id(&ctx.db).await {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"message": "Es läuft bereits eine Session."})),
        ));
    }

    // Pending Session holen
    let pending_sid = db::settings_get(&ctx.db, "sim.pending_session_id")
        .await
        .ok()
        .flatten()
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(json!({"message": "Kein Vor-Start aktiv."})),
        ))?;

    let result: Result<SimStartResponse, (StatusCode, Json<serde_json::Value>)> = async {
        let now = chrono::Utc::now().timestamp();
        let name = payload
            .name
            .clone()
            .unwrap_or_else(|| format!("Session_{}", now));

        // Session aktiv schalten + Startzeit setzen
        db::sim_set_active(&ctx.db, pending_sid, true, Some(now))
            .await
            .map_err(|e| {
                tracing::error!("sim_commit: konnte Session nicht aktivieren: {e:?}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"message": "Session konnte nicht aktiviert werden."})),
                )
            })?;

        // Aufnahme-Verzeichnis
        let dirs = db::app_dirs();
        let base_folder = dirs.rec_dir.join(pending_sid.to_string());
        fs::create_dir_all(&base_folder).map_err(|e| {
            tracing::error!(
                "sim_commit: create_dir_all({:?}) fehlgeschlagen: {e:?}",
                base_folder
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"message": "Aufnahme-Ordner konnte nicht erstellt werden."})),
            )
        })?;
        created_rec_dir = Some(base_folder.clone());
        db::settings_set(&ctx.db, "record.dir", &base_folder.to_string_lossy())
            .await
            .ok();

        // Record-Optionen laden
        let out_rtmp = db::settings_get(&ctx.db, "output.rtmp")
            .await
            .ok()
            .flatten();

        let mut audio_dev = db::settings_get(&ctx.db, "audio.device")
            .await
            .ok()
            .flatten();

        if let Some(room_id) = payload.room_id {
            let room = db::room_get(&ctx.db, room_id).await.map_err(|_| {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({"message": "Raum nicht gefunden."})),
                )
            })?;
            if let Some(dev) = room.audio_device.as_ref() {
                if !dev.trim().is_empty() {
                    audio_dev = Some(dev.trim().to_string());
                }
            }
        }


        let rec_enabled = db::settings_get(&ctx.db, "record.enabled")
            .await
            .ok()
            .flatten()
            .map(|v| v == "true")
            .unwrap_or(false);

        let basename = format!("session-{}", pending_sid);
        let rec_dir = base_folder.to_string_lossy().to_string();

        let split_ms = db::settings_get(&ctx.db, "record.segment_secs")
            .await
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(300)
            * 1000;

        let container = db::settings_get(&ctx.db, "record.format")
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "mp4".into());

        let max_files = db::settings_get(&ctx.db, "record.max_files")
            .await
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);

        let rec_audio_dev = audio_dev.clone();

        let rec_opts = if rec_enabled {
            Some(crate::pipeline::RecordOptions {
                enabled: true,
                directory: rec_dir.clone(),
                basename,
                container,
                split_ms,
                max_files,
                audio_dev: rec_audio_dev,
            })
        } else {
            None
        };

        // Pipeline neu bauen mit Recording an
        let st = ctx.status.lock().await.clone();
        {
            let mut pm = ctx.pipeline.lock().await;
            pm.rebuild(
                &st,
                out_rtmp.as_deref(),
                audio_dev.as_deref(),
                rec_opts,
            )
            .map_err(|e| {
                tracing::error!("sim_commit: Pipeline-Rebuild fehlgeschlagen: {e:?}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"message": "Pipeline konnte nicht gebaut werden (Start)."})),
                )
            })?;
        }
        // DASH optional anflanschen (wenn angefordert oder env aktiv)
        // Viewer triggern
        let mut effective_viewer_ids = if let Some(ids) = payload.viewer_ids.clone() {
            Some(ids)
        } else if let Some(id) = payload.viewer_id {
            Some(vec![id])
        } else {
            None
        };

        if effective_viewer_ids.is_none() {
            if let Some(room_id) = payload.room_id {
                if let Ok(room) = db::room_get(&ctx.db, room_id).await {
                    if let Some(v) = room.viewer_id {
                        effective_viewer_ids = Some(vec![v]);
                    }
                }
            }
        }

        // Viewer mit Map (Mode/Role) bevorzugt
        if let Some(map) = payload.viewer_map.as_ref() {
            let ws_path = format!("/api/webrtc/{}", pending_sid);
            for v in map {
                if v.id <= 0 {
                    continue;
                }
                let cmd = json!({
                    "cmd": "show_live",
                    "url": ws_path,
                });
                crate::routes::send_viewer_command(&ctx, v.id, cmd).await;
                let vol_cmd = json!({
                    "action": "set_volume",
                    "volume": 100
                });
                crate::routes::send_viewer_command(&ctx, v.id, vol_cmd).await;
            }
        } else if let Some(viewer_ids) = effective_viewer_ids {
            let ws_path = format!("/api/webrtc/{}", pending_sid);
            for viewer_id in viewer_ids {
                if viewer_id <= 0 {
                    continue;
                }
                let cmd = json!({
                    "cmd": "show_live",
                    "url": ws_path,
                });
                crate::routes::send_viewer_command(&ctx, viewer_id, cmd).await;

                let vol_cmd = json!({
                    "action": "set_volume",
                    "volume": 100
                });
                crate::routes::send_viewer_command(&ctx, viewer_id, vol_cmd).await;
            }
        } else {
            // Fallback: keine Auswahl → an alle verbundenen Viewer senden
            let ws_path = format!("/api/webrtc/{}", pending_sid);
            let cmd = json!({
                "cmd": "show_live",
                "url": ws_path,
            });
            crate::routes::broadcast_viewer_command(&ctx, cmd).await;
            let vol_cmd = json!({
                "action": "set_volume",
                "volume": 100
            });
            crate::routes::broadcast_viewer_command(&ctx, vol_cmd).await;
        }

        // pending Session löschen
        let _ = db::settings_set(&ctx.db, "sim.pending_session_id", "").await;
        // aktiven Raum merken (für Anzeige/Live-Raumwechsel)
        set_active_room_setting(&ctx, payload.room_id).await;

        // UI-Broadcast: sim_started
        {
            let msg = json!({
                "type": "sim_started",
                "session_id": pending_sid,
                "name": name,
                "record_is_active": rec_enabled,
                "start_time": now
            });
            broadcast_ui(&ctx, msg).await;
        }

        Ok(SimStartResponse {
            session_id: pending_sid,
            name,
            recording_folder: base_folder.to_string_lossy().to_string(),
            record_is_active: rec_enabled,
            start_time: now,
        })
    }
    .await;

    match result {
        Ok(resp) => {
            if let Some(uid) = user_id {
                let username = crate::db::user_by_id(&ctx.db, uid)
                    .await
                    .ok()
                    .flatten()
                    .map(|u| u.username);
                match crate::achievement::on_sim_started(&ctx.db, uid, resp.start_time).await {
                    Ok(keys) => {
                        if !keys.is_empty() {
                            if let Ok(defs) = crate::achievement::achievement_payloads_by_keys(&ctx.db, &keys).await {
                                let msg = json!({
                                    "type": "achievement_unlocked",
                                    "user_id": uid,
                                    "username": username,
                                    "achievements": defs,
                                });
                                broadcast_ui(&ctx, msg).await;
                            }
                        }
                    }
                    Err(e) => tracing::warn!(?e, user_id = uid, "achievement on_sim_started failed"),
                }
            }
            Ok(Json(resp))
        }
        Err(err) => {
            if let Some(dir) = created_rec_dir {
                let _ = fs::remove_dir_all(&dir);
            }
            let _ = db::settings_set(&ctx.db, "sim.pending_session_id", "").await;
            let _ = db::sim_delete_session(&ctx.db, pending_sid).await;
            Err(err)
        }
    }
}

/// Bricht einen offenen Vor-Start ab: Merker löschen, die nie aktivierte Session samt
/// (leerem) Aufnahmeordner entfernen, Zuschauer zurück auf Leerlauf. Die Pipeline stoppt
/// der Aufrufer selbst. Früher wurde der Merker nur bei „Start freigeben“ gelöscht – nach
/// einem Abbruch blieb jeder weitere Vor-Start mit „Es gibt bereits einen Vor-Start.“
/// gesperrt (auch nach Serverneustart), und ein späteres Freigeben hätte die alte Session
/// aktiviert.
pub async fn cancel_pending_prestart(ctx: &AppCtx, reason: &str) -> Option<i64> {
    let sid = db::settings_get(&ctx.db, "sim.pending_session_id")
        .await
        .ok()
        .flatten()
        .and_then(|s| s.trim().parse::<i64>().ok())?;
    let _ = db::settings_set(&ctx.db, "sim.pending_session_id", "").await;
    // Nur löschen, wenn die Session wirklich nie aktiviert wurde
    let active = db::sim_active_session_id(&ctx.db).await.ok().flatten();
    if active != Some(sid) {
        let _ = db::sim_delete_session(&ctx.db, sid).await;
        // Vor-Start nimmt nichts auf; remove_dir entfernt den Ordner nur, wenn er leer ist
        let _ = fs::remove_dir(db::app_dirs().rec_dir.join(sid.to_string()));
        crate::routes::broadcast_viewer_command(ctx, json!({ "cmd": "show_idle" })).await;
        crate::logging::emit_ui_event(json!({
            "type": "pre_start_cancelled",
            "session_id": sid,
            "ts": chrono::Utc::now().to_rfc3339(),
        }));
    }
    tracing::info!(session_id = sid, reason, "⏹️ Vor-Start abgebrochen");
    Some(sid)
}

/// ------------------------------------------------------------
/// POST /api/sim/prestart
/// ------------------------------------------------------------
pub async fn sim_prestart(
    State(ctx): State<AppCtx>,
    Json(payload): Json<SimPrestartPayload>,
) -> Result<Json<SimStartResponse>, (StatusCode, Json<serde_json::Value>)> {
    let mut created_session_id: Option<i64> = None;
    let mut created_rec_dir: Option<PathBuf> = None;

    // aktiv laufende Session verhindern
    if let Ok(Some(_sid)) = db::sim_active_session_id(&ctx.db).await {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"message": "Es läuft bereits eine Session."})),
        ));
    }
    // Ein noch offener Vor-Start wird durch den neuen ersetzt
    cancel_pending_prestart(&ctx, "neuer Vor-Start").await;

    let result: Result<SimStartResponse, (StatusCode, Json<serde_json::Value>)> = async {
        let now = chrono::Utc::now().timestamp();
        let name = payload
            .name
            .clone()
            .unwrap_or_else(|| format!("Session_{}", now));

        // Session anlegen, aber nicht aktiv
        let session_id = db::sim_insert_session_custom(&ctx.db, &name, 0, false)
            .await
            .map_err(|e| {
                tracing::error!("sim_prestart: konnte Session nicht anlegen: {e:?}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"message": "Session konnte nicht in der Datenbank angelegt werden."})),
                )
            })?;
        created_session_id = Some(session_id);

        // Webhook: Pre-Start
        crate::routes::trigger_webhook(
            "simulation_pre_start",
            serde_json::json!({ "session_id": session_id, "name": name }),
        )
        .await;

        // ----- ROOM-VARIABLEN -----
        let mut room_layout_code: Option<String> = None;
        let mut room_source_urls: Option<Vec<Option<String>>> = None;
        let mut room_audio_override: Option<String> = None;

        // ----- Room laden (optional) -----
        if let Some(room_id) = payload.room_id {
            let room = db::room_get(&ctx.db, room_id)
                .await
                .map_err(|_| (StatusCode::NOT_FOUND, Json(json!({"message": "Raum nicht gefunden."}))))?;

            tracing::info!("sim_prestart: apply room {} ('{}')", room.id, room.name);
            room_layout_code = Some(room.layout.clone());

            if let Some(dev) = room.audio_device.as_ref() {
                if !dev.trim().is_empty() {
                    room_audio_override = Some(dev.trim().to_string());
                }
            }

            // Quellen
            if let Some(src_json) = &room.sources {
                match serde_json::from_str::<Vec<Option<i64>>>(src_json) {
                    Ok(id_vec) => {
                        let mut urls = Vec::new();

                        for cam_id_opt in id_vec.into_iter().take(64) {
                            if let Some(cam_id) = cam_id_opt {
                                match db::camera_get(&ctx.db, cam_id).await {
                                    Ok(cam) => {
                                        let url = cam.url.trim().to_string();
                                        if url.is_empty() {
                                            urls.push(None);
                                        } else {
                                            urls.push(Some(url));
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!(
                                            "[sim] room: camera id {} not found: {:?}",
                                            cam_id, e
                                        );
                                        urls.push(None);
                                    }
                                }
                            } else {
                                urls.push(None);
                            }
                        }
                        room_source_urls = Some(urls);
                    }
                    Err(e) => {
                        eprintln!(
                            "[sim] room.sources JSON ungültig (room {}): {:?}",
                            room_id, e
                        );
                    }
                }
            }
        }

        // ----- Aufnahme-Verzeichnis vorbereiten -----
        let dirs = db::app_dirs();
        let base_folder = dirs.rec_dir.join(session_id.to_string());
        fs::create_dir_all(&base_folder).map_err(|e| {
            tracing::error!("sim_prestart: create_dir_all({:?}) fehlgeschlagen: {e:?}", base_folder);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"message": "Aufnahme-Ordner konnte nicht erstellt werden."})),
            )
        })?;
        created_rec_dir = Some(base_folder.clone());

        // ----- Record-Optionen (für prestart deaktiviert) -----
        let rec_opts = None;

        // ----- Audio-Device -----
        let mut audio_dev = db::settings_get(&ctx.db, "audio.device")
            .await
            .ok()
            .flatten();
        if let Some(dev) = room_audio_override {
            audio_dev = Some(dev);
        }

        tracing::info!("sim_prestart: set sources");
        // ----- Quellen setzen -----
        if let Some(urls) = room_source_urls {
            let len = urls.len();
            let mut pm = tokio::time::timeout(Duration::from_secs(2), ctx.pipeline.lock())
                .await
                .map_err(|_| {
                    tracing::warn!("sim_prestart: pipeline mutex busy (sources)");
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"message": "Pipeline busy (sources)."})),
                    )
                })?;
            for (i, url) in urls.into_iter().enumerate() {
                let _ = pm.set_source(i as i32, url);
            }
            for i in len..64 {
                let _ = pm.set_source(i as i32, None);
            }
        } else {
            let mut sources: Vec<Option<String>> = Vec::with_capacity(64);
            for i in 0..64 {
                let key = format!("source.{}", i);
                let url_opt = db::settings_get(&ctx.db, &key)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|s| {
                        let t = s.trim().to_string();
                        if t.is_empty() {
                            None
                        } else {
                            Some(t)
                        }
                    });
                sources.push(url_opt);
            }
            let mut pm = tokio::time::timeout(Duration::from_secs(2), ctx.pipeline.lock())
                .await
                .map_err(|_| {
                    tracing::warn!("sim_prestart: pipeline mutex busy (sources)");
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"message": "Pipeline busy (sources)."})),
                    )
                })?;
            for (i, decided) in sources.into_iter().enumerate() {
                let _ = pm.set_source(i as i32, decided);
            }
        }

        tracing::info!("sim_prestart: set layout");
        // ----- Layout setzen -----
        let layout_code = if let Some(code) = room_layout_code.clone() {
            Some(code)
        } else {
            db::settings_get(&ctx.db, "layout").await.ok().flatten()
        };
        {
            let mut pm = tokio::time::timeout(Duration::from_secs(2), ctx.pipeline.lock())
                .await
                .map_err(|_| {
                    tracing::warn!("sim_prestart: pipeline mutex busy (layout)");
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"message": "Pipeline busy (layout)."})),
                    )
                })?;
            if let Some(code) = layout_code {
                pm.set_cached_layout(build_layout_geoms(&code));
            } else {
                pm.set_cached_layout(build_layout_geoms("1"));
            }
        }

        tracing::info!("sim_prestart: rebuild pipeline");
        // ----- Pipeline-Rebuild (ohne Recording) -----
        let st = ctx.status.lock().await.clone();
        {
            let mut pm = tokio::time::timeout(Duration::from_secs(5), ctx.pipeline.lock())
                .await
                .map_err(|_| {
                    tracing::warn!("sim_prestart: pipeline mutex busy (rebuild)");
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"message": "Pipeline busy (rebuild)."})),
                    )
                })?;
            pm.rebuild(&st, None, audio_dev.as_deref(), rec_opts)
                .map_err(|e| {
                    tracing::error!("sim_prestart: Pipeline-Rebuild fehlgeschlagen: {e:?}");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"message": "Pipeline konnte nicht gebaut werden (Vor-Start)." })),
                    )
                })?;
        }
        // DASH optional anflanschen
        let dash_ok = false;

        // record.dir erst nach Erfolg setzen
        db::settings_set(&ctx.db, "record.dir", &base_folder.to_string_lossy())
            .await
            .ok();

        // pending Session merken – vor dem UI-Broadcast, damit /api/sim/status den
        // Vor-Start schon meldet, wenn Clients auf das Ereignis hin nachfragen
        let _ = db::settings_set(
            &ctx.db,
            "sim.pending_session_id",
            &session_id.to_string(),
        )
        .await;

        // UI-Broadcast: pre_start info
        {
            let msg = json!({
                "type": "pre_start",
                "session_id": session_id,
                "name": name,
            });
            broadcast_ui(&ctx, msg).await;
        }

        // Zuschauer/alle Viewer informieren: Start steht bevor (kein Video, kein Recording).
        // Wird an alle verbundenen Viewer geschickt; Ausbilder werden direkt danach per
        // show_live auf den Live-Screen umgeschaltet und sehen den Hinweis daher nicht lange.
        {
            let notice_cmd = json!({
                "cmd": "pre_start_notice",
                "session_id": session_id,
                "name": name,
            });
            crate::routes::broadcast_viewer_command(&ctx, notice_cmd).await;
        }

        // Ausbilder-Viewer informieren (soft)
        let mut instr_ids: Option<Vec<i64>> = payload.instructor_ids.clone();
        if instr_ids.is_none() {
            if let Some(map) = &payload.viewer_map {
                let ids: Vec<i64> = map
                    .iter()
                    .filter(|v| {
                        v.role.as_deref() == Some("instructor")
                            || (v.role.is_none()
                                && viewer_role_from_id(v.id) == "instructor")
                    })
                    .map(|v| v.id)
                    .collect();
                if !ids.is_empty() {
                    instr_ids = Some(ids);
                }
            }
        }

        if let Some(instr_ids) = instr_ids.as_ref() {
            let cmd = json!({
                "cmd": "pre_start",
                "session_id": session_id,
            });
            for vid in instr_ids {
                crate::routes::send_viewer_command(&ctx, *vid, cmd.clone()).await;

                // Instruktoren sofort den Stream zeigen
                let stream_mode = payload
                    .viewer_map
                    .as_ref()
                    .and_then(|m| m.iter().find(|v| v.id == *vid))
                    .and_then(|v| v.mode.clone())
                    .unwrap_or_else(|| "webrtc".into());
                let ws_path = format!("/api/webrtc/{}", session_id);
                let dash = dash_url(session_id);
                let cmd = if stream_mode.eq_ignore_ascii_case("dash") && dash_ok {
                    json!({ "cmd": "show_live", "url": dash })
                } else {
                    json!({ "cmd": "show_live", "url": ws_path })
                };
                crate::routes::send_viewer_command(&ctx, *vid, cmd).await;
            }
        }


        Ok(SimStartResponse {
            session_id,
            name,
            recording_folder: base_folder.to_string_lossy().to_string(),
            record_is_active: false,
            start_time: now,
        })
    }
    .await;

    match result {
        Ok(resp) => Ok(Json(resp)),
        Err(err) => {
            if let Some(session_id) = created_session_id {
                let _ = db::sim_delete_session(&ctx.db, session_id).await;
            }
            if let Some(dir) = created_rec_dir {
                let _ = fs::remove_dir_all(&dir);
            }
            let _ = db::settings_set(&ctx.db, "sim.pending_session_id", "").await;
            Err(err)
        }
    }
}

/// Stop einer Simulation/Session
#[derive(Deserialize)]
pub struct SimStopPayload {
    /// optional, wenn leer -> active session
    pub session_id: Option<i64>,
    pub viewer_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub viewer_id: Option<i64>, // Backward-Compat
}

#[derive(Serialize)]
pub struct SimStopResponse {
    pub session_id: i64,
    pub length_ms: i64,
    pub vod_file: Option<String>,
}

/// Debrief-Position setzen/holen
#[derive(Deserialize)]
pub struct DebriefPositionSet {
    pub position_ms: i64,
}

#[derive(Serialize)]
pub struct DebriefPositionGet {
    pub position_ms: i64,
    pub length_ms: i64,
    pub file: Option<String>,
}

// Canvas annehmen (du kannst die Werte später zentralisieren)
const CW: i32 = 1920;
const CH: i32 = 1080;

/// Erzeugt Slot-Geometrien für ein paar Standard-Layouts.
/// Rückgabe: Vektor in derselben Reihenfolge, wie die Sinks (sink_0, sink_1, …)
fn build_layout_geoms(code: &str) -> Vec<SlotGeom> {
    match code.trim().to_lowercase().as_str() {
        // 1 primary links (2/3 Breite), rechts 3 Thumbs übereinander
        "1p+3" | "1+3" => {
            let col_r_w = CW / 3; // rechte Spalte
            let col_l_w = CW - col_r_w; // linke Spalte (2/3)
            let thumb_h = CH / 3;

            vec![
                // sink_0 – Primary (groß, links)
                SlotGeom {
                    x: 0,
                    y: 0,
                    w: col_l_w,
                    h: CH,
                    z: 1,
                },
                // sink_1..3 – Thumbs (rechts)
                SlotGeom {
                    x: col_l_w,
                    y: 0,
                    w: col_r_w,
                    h: thumb_h,
                    z: 2,
                },
                SlotGeom {
                    x: col_l_w,
                    y: thumb_h,
                    w: col_r_w,
                    h: thumb_h,
                    z: 2,
                },
                SlotGeom {
                    x: col_l_w,
                    y: 2 * thumb_h,
                    w: col_r_w,
                    h: CH - 2 * thumb_h,
                    z: 2,
                },
            ]
        }

        // Gleichmäßiges 2x2
        "2x2" => {
            let w = CW / 2;
            let h = CH / 2;
            vec![
                SlotGeom {
                    x: 0,
                    y: 0,
                    w,
                    h,
                    z: 0,
                }, // sink_0
                SlotGeom {
                    x: w,
                    y: 0,
                    w,
                    h,
                    z: 1,
                }, // sink_1
                SlotGeom {
                    x: 0,
                    y: h,
                    w,
                    h,
                    z: 2,
                }, // sink_2
                SlotGeom {
                    x: w,
                    y: h,
                    w,
                    h,
                    z: 3,
                }, // sink_3
            ]
        }

        // 3x3 Grid (bis 9 Quellen)
        "3x3" => {
            let w = CW / 3;
            let h = CH / 3;
            let mut v = Vec::with_capacity(9);
            for r in 0..3 {
                for c in 0..3 {
                    v.push(SlotGeom {
                        x: c * w,
                        y: r * h,
                        w,
                        h,
                        z: 1,
                    });
                }
            }
            v
        }

        // 1 groß + 5 kleine (oben links groß; unten 5 Kacheln)
        "1+5" => {
            let big_w = CW * 2 / 3;
            let big_h = CH * 2 / 3;
            let rem_w = CW - big_w;
            let rem_h = CH - big_h;

            // 5 Kacheln im verbleibenden Bereich grob verteilen
            let k_w = rem_w.max(1);
            let k_h = rem_h / 3; // 3 Zeilen rechts
            vec![
                SlotGeom {
                    x: 0,
                    y: 0,
                    w: big_w,
                    h: big_h,
                    z: 1,
                }, // sink_0 (primary)
                SlotGeom {
                    x: big_w,
                    y: 0,
                    w: k_w,
                    h: k_h,
                    z: 2,
                }, // sink_1
                SlotGeom {
                    x: big_w,
                    y: k_h,
                    w: k_w,
                    h: k_h,
                    z: 2,
                }, // sink_2
                SlotGeom {
                    x: big_w,
                    y: 2 * k_h,
                    w: k_w,
                    h: CH - 2 * k_h,
                    z: 2,
                }, // sink_3
                SlotGeom {
                    x: 0,
                    y: big_h,
                    w: big_w / 2,
                    h: CH - big_h,
                    z: 2,
                }, // sink_4
                SlotGeom {
                    x: big_w / 2,
                    y: big_h,
                    w: big_w - big_w / 2,
                    h: CH - big_h,
                    z: 2,
                }, // sink_5
            ]
        }

        // 3-Kameras: 1 groß, 2 überlagerte PIP rechts unten
        "3pip" | "3pip-br" => {
            let pip_w = CW / 4;
            let pip_h = CH / 4;
            vec![
                SlotGeom {
                    x: 0,
                    y: 0,
                    w: CW,
                    h: CH,
                    z: 1,
                }, // sink_0 groß
                SlotGeom {
                    x: CW - pip_w - 20,
                    y: CH - pip_h - 20,
                    w: pip_w,
                    h: pip_h,
                    z: 2,
                }, // sink_1 PIP unten rechts
                SlotGeom {
                    x: CW - pip_w - 20,
                    y: CH - 2 * pip_h - 40,
                    w: pip_w,
                    h: pip_h,
                    z: 3,
                }, // sink_2 PIP oben drüber
            ]
        }

        // 3-Kameras: gestapelt diagonal (leicht überlappend)
        "3diag" | "3layer" => {
            let w = CW * 3 / 5;
            let h = CH * 3 / 5;
            vec![
                SlotGeom {
                    x: 0,
                    y: 0,
                    w,
                    h,
                    z: 1,
                }, // sink_0
                SlotGeom {
                    x: CW / 5,
                    y: CH / 5,
                    w,
                    h,
                    z: 2,
                }, // sink_1 überlappt leicht
                SlotGeom {
                    x: CW * 2 / 5,
                    y: CH * 2 / 5,
                    w,
                    h,
                    z: 3,
                }, // sink_2 oben
            ]
        }

        // Single-View
        "1" | "1p" | "single" | "" => {
            vec![SlotGeom {
                x: 0,
                y: 0,
                w: CW,
                h: CH,
                z: 1,
            }]
        }

        // Fallback → Single
        _ => {
            eprintln!("⚠️ unbekanntes Layout '{}', fallback auf Single", code);
            vec![SlotGeom {
                x: 0,
                y: 0,
                w: CW,
                h: CH,
                z: 1,
            }]
        }
    }
}

/// ------------------------------------------------------------
/// POST /api/sim/start
/// ------------------------------------------------------------
pub async fn sim_start(
    State(ctx): State<AppCtx>,
    headers: HeaderMap,
    Json(payload): Json<SimStartPayload>,
) -> Result<Json<SimStartResponse>, (StatusCode, Json<serde_json::Value>)> {
    let mut created_session_id: Option<i64> = None;
    let mut created_rec_dir: Option<PathBuf> = None;
    let user_id = crate::achievement::auth_user_id_from_headers(&ctx, &headers);

    // 1) Prüfen, ob bereits eine Session läuft
    if let Ok(Some(_sid)) = db::sim_active_session_id(&ctx.db).await {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"message": "Es läuft bereits eine Session."})),
        ));
    }
    // Direkter Start ersetzt einen offenen Vor-Start (Pipeline wird gleich neu gebaut)
    cancel_pending_prestart(&ctx, "direkter Start").await;

    let result: Result<SimStartResponse, (StatusCode, Json<serde_json::Value>)> = async {
        // 2) Neue Session anlegen
        let now = chrono::Utc::now().timestamp();
        let name = payload
            .name
            .clone()
            .unwrap_or_else(|| format!("Session_{}", now));

        let session_id = db::sim_insert_session_start(&ctx.db, &name, now)
            .await
            .map_err(|e| {
                tracing::error!("sim_start: konnte Session nicht in DB anlegen: {e:?}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"message": "Session konnte nicht in der Datenbank angelegt werden."})),
                )
            })?;
        created_session_id = Some(session_id);

        // Webhook: Session gestartet (erst nach erfolgreichem Pipeline-Build)

        // ----- ROOM-VARIABLEN -----
        let mut effective_viewer_ids = if let Some(ids) = payload.viewer_ids.clone() {
            Some(ids)
        } else if let Some(id) = payload.viewer_id {
            Some(vec![id])
        } else {
            None
        };
        let mut room_layout_code: Option<String> = None;
        let mut room_source_urls: Option<Vec<Option<String>>> = None;
        let mut room_audio_override: Option<String> = None;

        // ----- 3) Falls ein Room übergeben wurde: laden -----
        if let Some(room_id) = payload.room_id {
            let room = db::room_get(&ctx.db, room_id)
                .await
                .map_err(|_| (StatusCode::NOT_FOUND, Json(json!({"message": "Raum nicht gefunden."}))))?;

            tracing::info!("sim_start: apply room {} ('{}')", room.id, room.name);

            // Layout aus Room
            room_layout_code = Some(room.layout.clone());

            // Audio-Override aus Room
            if let Some(dev) = room.audio_device.as_ref() {
                if !dev.trim().is_empty() {
                    room_audio_override = Some(dev.trim().to_string());
                }
            }

            // Default-Viewer (falls nicht im Payload)
            if effective_viewer_ids.is_none() {
                effective_viewer_ids = room.viewer_id.map(|id| vec![id]);
            }

            // Quellen: JSON = [camera_id|null, ...]
            if let Some(src_json) = &room.sources {
                match serde_json::from_str::<Vec<Option<i64>>>(src_json) {
                    Ok(id_vec) => {
                        let mut urls = Vec::new();

                        for cam_id_opt in id_vec.into_iter().take(64) {
                            if let Some(cam_id) = cam_id_opt {
                                match db::camera_get(&ctx.db, cam_id).await {
                                    Ok(cam) => {
                                        let url = cam.url.trim().to_string();
                                        if url.is_empty() {
                                            // Keine URL → explizit Fallback/Testbild
                                            urls.push(None);
                                        } else {
                                            // ✅ Kein Reachability-Check mehr – GStreamer entscheidet später
                                            urls.push(Some(url));
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!(
                                            "[sim] room: camera id {} not found: {:?}",
                                            cam_id, e
                                        );
                                        urls.push(None);
                                    }
                                }
                            } else {
                                urls.push(None);
                            }
                        }

                        room_source_urls = Some(urls);
                    }
                    Err(e) => {
                        eprintln!(
                            "[sim] room.sources JSON ungültig (room {}): {:?}",
                            room_id, e
                        );
                    }
                }
            }
        }

        // ----- 4) Aufnahme-Verzeichnis vorbereiten -----
        let dirs = db::app_dirs();
        let base_folder = dirs.rec_dir.join(session_id.to_string());
        fs::create_dir_all(&base_folder).map_err(|e| {
            tracing::error!("sim_start: create_dir_all({:?}) fehlgeschlagen: {e:?}", base_folder);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"message": "Aufnahme-Ordner konnte nicht erstellt werden."})),
            )
        })?;
        created_rec_dir = Some(base_folder.clone());

        // ----- 5) Record-Optionen laden -----
        let out_rtmp = db::settings_get(&ctx.db, "output.rtmp")
            .await
            .ok()
            .flatten();

        let mut audio_dev = db::settings_get(&ctx.db, "audio.device")
            .await
            .ok()
            .flatten();

        // Raum kann audio.device überschreiben
        if let Some(dev) = room_audio_override {
            audio_dev = Some(dev);
        }


        let rec_enabled = db::settings_get(&ctx.db, "record.enabled")
            .await
            .ok()
            .flatten()
            .map(|v| v == "true")
            .unwrap_or(false);

        let basename = format!("session-{}", session_id);
        let rec_dir = base_folder.to_string_lossy().to_string();

        let split_ms = db::settings_get(&ctx.db, "record.segment_secs")
            .await
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(300)
            * 1000;

        let container = db::settings_get(&ctx.db, "record.format")
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "mp4".into());

        let max_files = db::settings_get(&ctx.db, "record.max_files")
            .await
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);

        let rec_audio_dev = audio_dev.clone();

        let rec_opts = if rec_enabled {
            Some(crate::pipeline::RecordOptions {
                enabled: true,
                directory: rec_dir.clone(),
                basename,
                container,
                split_ms,
                max_files,
                audio_dev: rec_audio_dev,
            })
        } else {
            None
        };

        tracing::info!("sim_start: webrtc shutdown broadcast (pre-start)");
        let _ = ctx.webrtc_shutdown.send(());
        tokio::time::sleep(Duration::from_millis(200)).await;

        tracing::info!("sim_start: hard reset pipeline");
        {
            let mut pm = tokio::time::timeout(Duration::from_secs(2), ctx.pipeline.lock())
                .await
                .map_err(|_| {
                    tracing::warn!("sim_start: pipeline mutex busy (reset)");
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"message": "Pipeline busy (reset)."})),
                    )
                })?;
            pm.hard_reset().map_err(|e| {
                tracing::error!("sim_start: pipeline hard reset failed: {e:?}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"message": "Pipeline Reset fehlgeschlagen."})),
                )
            })?;
        }

        tracing::info!("sim_start: set sources");
        // ----- 6) Quellen setzen -----
        if let Some(urls) = room_source_urls {
            let len = urls.len();

            let mut pm = tokio::time::timeout(Duration::from_secs(2), ctx.pipeline.lock())
                .await
                .map_err(|_| {
                    tracing::warn!("sim_start: pipeline mutex busy (sources)");
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"message": "Pipeline busy (sources)."})),
                    )
                })?;

            // Room-Quellen
            for (i, url) in urls.into_iter().enumerate() {
                let _ = pm.set_source(i as i32, url);
            }

            // Restliche Slots auf Testbild
            for i in len..64 {
                let _ = pm.set_source(i as i32, None);
            }
        } else {
            // Fallback: alte Settings source.N
            let mut sources: Vec<Option<String>> = Vec::with_capacity(64);
            for i in 0..64 {
                let key = format!("source.{}", i);
                let url_opt = db::settings_get(&ctx.db, &key)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|s| {
                        let t = s.trim().to_string();
                        if t.is_empty() {
                            None
                        } else {
                            Some(t)
                        }
                    });

                // ✅ Kein Reachability-Check mehr
                sources.push(url_opt);
            }

            let mut pm = tokio::time::timeout(Duration::from_secs(2), ctx.pipeline.lock())
                .await
                .map_err(|_| {
                    tracing::warn!("sim_start: pipeline mutex busy (sources)");
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"message": "Pipeline busy (sources)."})),
                    )
                })?;
            for (i, decided) in sources.into_iter().enumerate() {
                let _ = pm.set_source(i as i32, decided);
            }
        }

        tracing::info!("sim_start: set layout");
        // ----- 7) Layout setzen -----
        let layout_code = if let Some(code) = room_layout_code.clone() {
            Some(code)
        } else {
            db::settings_get(&ctx.db, "layout").await.ok().flatten()
        };
        {
            let mut pm = tokio::time::timeout(Duration::from_secs(2), ctx.pipeline.lock())
                .await
                .map_err(|_| {
                    tracing::warn!("sim_start: pipeline mutex busy (layout)");
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"message": "Pipeline busy (layout)."})),
                    )
                })?;

            if let Some(code) = layout_code {
                pm.set_cached_layout(build_layout_geoms(&code));
            } else {
                pm.set_cached_layout(build_layout_geoms("1"));
            }
        }

        tracing::info!("sim_start: rebuild pipeline");
        // ----- 8) Pipeline-Rebuild -----
        let st = ctx.status.lock().await.clone();

        {
            let mut pm = tokio::time::timeout(Duration::from_secs(5), ctx.pipeline.lock())
                .await
                .map_err(|_| {
                    tracing::warn!("sim_start: pipeline mutex busy (rebuild)");
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"message": "Pipeline busy (rebuild)."})),
                    )
                })?;

            pm.rebuild(
                &st,
                out_rtmp.as_deref(),
                audio_dev.as_deref(),
                rec_opts,
            )
            .map_err(|e| {
                tracing::error!("sim_start: Pipeline-Rebuild fehlgeschlagen: {e:?}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"message": "Pipeline konnte nicht gebaut werden."})),
                )
            })?;
            let state = pm
                .pipeline
                .as_ref()
                .map(|p| format!("{:?}", p.current_state()))
                .unwrap_or_else(|| "None".to_string());
            tracing::info!("sim_start: pipeline state after rebuild = {}", state);
        }

        {
            let msg = json!({
                "type": "sim_start_wait",
                "session_id": session_id,
                "message": "Pipeline wird vorbereitet…",
            });
            broadcast_ui(&ctx, msg).await;
        }

        let playing = wait_for_pipeline_playing(&ctx, Duration::from_secs(3)).await;
        if playing {
            tracing::info!("sim_start: pipeline is Playing (ready for viewers)");
        } else {
            tracing::warn!("sim_start: pipeline not Playing after wait; continuing");
        }
        {
            let msg = json!({
                "type": "sim_start_ready",
                "session_id": session_id,
                "playing": playing,
                "message": if playing { "Pipeline bereit – Viewer verbinden…" } else { "Pipeline nicht bereit – fahre fort…" },
            });
            broadcast_ui(&ctx, msg).await;
        }

        // record.dir erst setzen, wenn alles erfolgreich war
        db::settings_set(&ctx.db, "record.dir", &base_folder.to_string_lossy())
            .await
            .ok();

        // Aktive Viewer-Liste für Autorejoin merken
        let mut active_viewers: Vec<i64> = Vec::new();

        // ----- 9) Viewer aus Room oder Payload antriggern -----
        if let Some(map) = payload.viewer_map.as_ref() {
            let ws_path = format!("/api/webrtc/{}", session_id);
            for v in map {
                if v.id <= 0 {
                    continue;
                }
                active_viewers.push(v.id);
                let reset_cmd = json!({
                    "cmd": "reset_webrtc"
                });
                crate::routes::send_viewer_command(&ctx, v.id, reset_cmd).await;
                let cmd = json!({
                    "cmd": "show_live",
                    "url": ws_path,
                });
                crate::routes::send_viewer_command(&ctx, v.id, cmd).await;

                // Versuch, den Viewer direkt mit Ton zu starten (falls Browser-Autoplay es zulässt)
                let vol_cmd = json!({
                    "action": "set_volume",
                    "volume": 100
                });
                crate::routes::send_viewer_command(&ctx, v.id, vol_cmd).await;
            }
        } else if let Some(viewer_ids) = effective_viewer_ids {
            let ws_path = format!("/api/webrtc/{}", session_id);
            for viewer_id in viewer_ids {
                if viewer_id <= 0 {
                    continue;
                }
                active_viewers.push(viewer_id);
                let reset_cmd = json!({
                    "cmd": "reset_webrtc"
                });
                crate::routes::send_viewer_command(&ctx, viewer_id, reset_cmd).await;
                let cmd = json!({
                    "cmd": "show_live",
                    "url": ws_path,
                });
                crate::routes::send_viewer_command(&ctx, viewer_id, cmd).await;

                // Versuch, den Viewer direkt mit Ton zu starten (falls Browser-Autoplay es zulässt)
                let vol_cmd = json!({
                    "action": "set_volume",
                    "volume": 100
                });
                crate::routes::send_viewer_command(&ctx, viewer_id, vol_cmd).await;
            }
        } else {
            // Fallback: keine Auswahl → an alle aktuell verbundenen Viewer senden
            let ws_path = format!("/api/webrtc/{}", session_id);
            let reset_cmd = json!({
                "cmd": "reset_webrtc"
            });
            crate::routes::broadcast_viewer_command(&ctx, reset_cmd).await;
            let cmd = json!({
                "cmd": "show_live",
                "url": ws_path,
            });
            crate::routes::broadcast_viewer_command(&ctx, cmd).await;
        }

        // Aktive Viewer in Settings sichern (für Autorejoin bei Reload)
        if let Ok(list) = serde_json::to_string(&active_viewers) {
            let _ = db::settings_set(&ctx.db, "sim.active_viewers", &list).await;
        }
        // aktiven Raum merken (für Anzeige/Live-Raumwechsel)
        set_active_room_setting(&ctx, payload.room_id).await;

        // Webhook: Session gestartet (erst jetzt, wenn alles OK)
        crate::routes::trigger_webhook(
            "session_start",
            serde_json::json!({ "session_id": session_id, "name": name }),
        )
        .await;

        // UI-Broadcast: sim_started (legacy start)
        {
            let msg = json!({
                "type": "sim_started",
                "session_id": session_id,
                "name": name,
                "record_is_active": rec_enabled,
                "start_time": now
            });
            broadcast_ui(&ctx, msg).await;
        }

        // ----- 10) Response -----
        Ok(SimStartResponse {
            session_id,
            name,
            recording_folder: base_folder.to_string_lossy().to_string(),
            record_is_active: rec_enabled,
            start_time: now,
        })
    }
    .await;

    match result {
        Ok(resp) => {
            if let Some(uid) = user_id {
                let username = crate::db::user_by_id(&ctx.db, uid)
                    .await
                    .ok()
                    .flatten()
                    .map(|u| u.username);
                match crate::achievement::on_sim_started(&ctx.db, uid, resp.start_time).await {
                    Ok(keys) => {
                        if !keys.is_empty() {
                            if let Ok(defs) = crate::achievement::achievement_payloads_by_keys(&ctx.db, &keys).await {
                                let msg = json!({
                                    "type": "achievement_unlocked",
                                    "user_id": uid,
                                    "username": username,
                                    "achievements": defs,
                                });
                                broadcast_ui(&ctx, msg).await;
                            }
                        }
                    }
                    Err(e) => tracing::warn!(?e, user_id = uid, "achievement on_sim_started failed"),
                }
            }
            Ok(Json(resp))
        }
        Err(err) => {
            if let Some(session_id) = created_session_id {
                let _ = db::sim_delete_session(&ctx.db, session_id).await;
            }
            if let Some(dir) = created_rec_dir {
                let _ = fs::remove_dir_all(&dir);
            }
            Err(err)
        }
    }
}

/// ------------------------------------------------------------
/// POST /api/sim/stop
/// ------------------------------------------------------------
pub async fn sim_stop(
    State(ctx): State<AppCtx>,
    headers: HeaderMap,
    Json(p): Json<SimStopPayload>,
) -> Result<Json<SimStopResponse>, axum::http::StatusCode> {
    let user_id = crate::achievement::auth_user_id_from_headers(&ctx, &headers);
    // 1) Session-Id bestimmen
    let active_sid = db::sim_active_session_id(&ctx.db)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    if p.session_id.is_none() && active_sid.is_none() {
        // Stopp während des Vor-Starts = Vor-Start abbrechen
        stop_pipeline_soft(&ctx, "sim_stop (Vor-Start)").await;
        let pending = cancel_pending_prestart(&ctx, "Stopp").await;
        return match pending {
            Some(sid) => Ok(Json(SimStopResponse { session_id: sid, length_ms: 0, vod_file: None })),
            None => Err(axum::http::StatusCode::NOT_FOUND),
        };
    }
    let sid = match p.session_id {
        Some(id) => id,
        None => active_sid.ok_or(axum::http::StatusCode::NOT_FOUND)?,
    };

    // 1a) Viewer auf IDLE schalten (optional)
    let viewer_ids: Option<Vec<i64>> = if let Some(v) = p.viewer_ids {
        Some(v)
    } else if let Some(id) = p.viewer_id {
        Some(vec![id])
    } else {
        None
    };

    if let Some(viewer_ids) = viewer_ids {
        let cmd = json!({
            "cmd": "show_idle"
        });

        for viewer_id in viewer_ids {
            if viewer_id <= 0 {
                continue;
            }
            crate::routes::send_viewer_command(&ctx, viewer_id, cmd.clone()).await;
            tracing::info!(
                "sim_stop: show_idle an Viewer {} geschickt (Session #{})",
                viewer_id,
                sid
            );
        }
    }
    // 2) Pipeline „sanft“ stoppen (oder Recording aus)
    stop_pipeline_soft(&ctx, "sim_stop").await;

    // aktive Viewer-Liste leeren
    let _ = db::settings_set(&ctx.db, "sim.active_viewers", "[]").await;
    set_active_room_setting(&ctx, None).await;

    // 3) Dauer / VOD-Datei ermitteln & DB finalisieren
    let now = chrono::Utc::now().timestamp();
    let start = db::sim_session_start_time(&ctx.db, sid)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;
    let length = (now - start).max(0) as i64 * 1000;
    //Transcode VOD
    let vod_name = guess_vod_file(&ctx.db, sid)
        .await
        .unwrap_or_else(|| format!("session-{}.mkv", sid));

    let dirs = db::app_dirs();
    let rec_dir = dirs.rec_dir.clone();
    let session_folder = rec_dir.join(sid.to_string());
    let vod_dir = rec_dir.join("vod");
    let _ = std::fs::create_dir_all(&vod_dir);

    // Input-Muster für ffmpeg, passend zu splitmuxsink:
    let input_pattern = session_folder.join(format!("session-{}_rec_%05d.mkv", sid));
    let input_pattern_str = input_pattern.to_string_lossy().to_string();

    // Output-Datei
    let out_dir_str = vod_dir.to_string_lossy().to_string();

    // Transcode-Job enqueuen
    let _job_id = db::job_enqueue(&ctx.db, sid, &input_pattern_str, &out_dir_str, "mp4")
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    // Session in DB finalisieren (wie oben, mit vod_name)
    db::sim_finalize_session(&ctx.db, sid, now, length, Some(&vod_name))
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let _ = db::settings_set(&ctx.db, "record.enabled", "false").await;

    // Webhook: Session gestoppt (non-blocking)
    let hook_vod = vod_name.clone();
    let _ = tokio::spawn(crate::routes::trigger_webhook(
        "session_stop",
        serde_json::json!({ "session_id": sid, "length_ms": length, "vod_file": hook_vod }),
    ));

    // UI-Broadcast: sim_stopped
    {
        let msg = json!({
            "type": "sim_stopped",
            "session_id": sid,
            "length_ms": length,
            "vod_file": vod_name,
        });
        broadcast_ui(&ctx, msg).await;
    }

    if let Some(uid) = user_id {
        let username = crate::db::user_by_id(&ctx.db, uid)
            .await
            .ok()
            .flatten()
            .map(|u| u.username);
        match crate::achievement::on_sim_stopped(&ctx.db, uid, length).await {
            Ok(keys) => {
                if !keys.is_empty() {
                    if let Ok(defs) = crate::achievement::achievement_payloads_by_keys(&ctx.db, &keys).await {
                        let msg = json!({
                            "type": "achievement_unlocked",
                            "user_id": uid,
                            "username": username,
                            "achievements": defs,
                        });
                        broadcast_ui(&ctx, msg).await;
                    }
                }
            }
            Err(e) => tracing::warn!(?e, user_id = uid, "achievement on_sim_stopped failed"),
        }
    }

    Ok(Json(SimStopResponse {
        session_id: sid,
        length_ms: length,
        vod_file: Some(vod_name),
    }))
}

/// ------------------------------------------------------------
/// GET /api/sim/status
/// Gibt aktive Session zurück (falls vorhanden)
/// ------------------------------------------------------------
#[derive(Serialize)]
pub struct SimStatus {
    pub active: bool,
    pub session_id: Option<i64>,
    pub name: Option<String>,
    pub start_time: Option<i64>,
    /// Aktuell aktiver Raum (falls die Session mit Raum gestartet oder live gewechselt wurde)
    pub room_id: Option<i64>,
    pub room_name: Option<String>,
    /// Session-Id eines laufenden Vor-Starts (noch nicht freigegeben)
    pub prestart_session_id: Option<i64>,
}
pub async fn sim_status(State(ctx): State<AppCtx>) -> Json<SimStatus> {
    let prestart_session_id = db::settings_get(&ctx.db, "sim.pending_session_id")
        .await
        .ok()
        .flatten()
        .and_then(|s| s.trim().parse::<i64>().ok());
    if let Ok(Some(row)) = db::sim_active_session_row(&ctx.db).await {
        let room_id = active_room_id_setting(&ctx).await;
        let room_name = match room_id {
            Some(id) => db::room_get(&ctx.db, id).await.ok().map(|r| r.name),
            None => None,
        };
        Json(SimStatus {
            active: true,
            session_id: Some(row.id),
            name: row.name,
            start_time: row.start_time,
            room_id,
            room_name,
            prestart_session_id,
        })
    } else {
        Json(SimStatus {
            active: false,
            session_id: None,
            name: None,
            start_time: None,
            room_id: None,
            room_name: None,
            prestart_session_id,
        })
    }
}

/// Setting `sim.active_room_id` schreiben (leer = kein Raum).
async fn set_active_room_setting(ctx: &AppCtx, room_id: Option<i64>) {
    let val = room_id.map(|id| id.to_string()).unwrap_or_default();
    let _ = db::settings_set(&ctx.db, "sim.active_room_id", &val).await;
}

async fn active_room_id_setting(ctx: &AppCtx) -> Option<i64> {
    db::settings_get(&ctx.db, "sim.active_room_id")
        .await
        .ok()
        .flatten()
        .and_then(|s| s.trim().parse::<i64>().ok())
}

/// Raum-Quellen (JSON-Array von Kamera-IDs) zu URLs auflösen. Unbekannte Kameras
/// oder leere URLs ergeben `None` (Slot zeigt Fallback).
async fn resolve_room_source_urls(ctx: &AppCtx, room: &db::RoomRow) -> Vec<Option<String>> {
    let Some(src_json) = room.sources.as_deref() else {
        return Vec::new();
    };
    let id_vec: Vec<Option<i64>> = match serde_json::from_str(src_json) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(room_id = room.id, ?e, "room.sources JSON ungültig");
            return Vec::new();
        }
    };
    let mut urls = Vec::with_capacity(id_vec.len());
    for cam_id_opt in id_vec.into_iter().take(64) {
        let Some(cam_id) = cam_id_opt else {
            urls.push(None);
            continue;
        };
        match db::camera_get(&ctx.db, cam_id).await {
            Ok(cam) => {
                let url = cam.url.trim().to_string();
                urls.push(if url.is_empty() { None } else { Some(url) });
            }
            Err(e) => {
                tracing::warn!(room_id = room.id, cam_id, ?e, "room: camera not found");
                urls.push(None);
            }
        }
    }
    urls
}

/// ------------------------------------------------------------
/// POST /api/sim/switch_room
/// Wechselt den Raum (Layout + Kameraquellen + Viewer) während einer laufenden
/// Session, ohne die Pipeline neu zu bauen. Aufnahme und Viewer-Stream laufen
/// dabei ununterbrochen weiter; getauschte Slots zeigen kurz das Schwarzbild,
/// bis die neue Kamera das erste Bild liefert.
/// ------------------------------------------------------------
#[derive(Deserialize)]
pub struct SwitchRoomPayload {
    pub room_id: i64,
}

#[derive(Serialize)]
pub struct SwitchRoomResponse {
    pub ok: bool,
    pub session_id: i64,
    pub room_id: i64,
    pub room_name: String,
    pub layout: String,
    pub warnings: Vec<String>,
}

pub async fn sim_switch_room(
    State(ctx): State<AppCtx>,
    headers: HeaderMap,
    Json(payload): Json<SwitchRoomPayload>,
) -> Result<Json<SwitchRoomResponse>, (StatusCode, Json<serde_json::Value>)> {
    let user_id = crate::achievement::auth_user_id_from_headers(&ctx, &headers);

    // 1) Aktive Session
    let session = db::sim_active_session_row(&ctx.db)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            (
                StatusCode::CONFLICT,
                Json(json!({"message": "Keine aktive Session – Raumwechsel nur im laufenden Betrieb."})),
            )
        })?;
    let session_id = session.id;

    // 2) Raum laden + Quellen auflösen
    let room = db::room_get(&ctx.db, payload.room_id).await.map_err(|_| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"message": "Raum nicht gefunden."})),
        )
    })?;
    let previous_room_id = active_room_id_setting(&ctx).await;
    let urls = resolve_room_source_urls(&ctx, &room).await;
    let geoms = build_layout_geoms(&room.layout);
    tracing::info!(
        session_id,
        room_id = room.id,
        room = %room.name,
        layout = %room.layout,
        sources = urls.len(),
        "sim_switch_room: start"
    );

    // 3) Pipeline live umschalten
    let warnings = {
        let mut pm = tokio::time::timeout(Duration::from_secs(5), ctx.pipeline.lock())
            .await
            .map_err(|_| {
                tracing::warn!("sim_switch_room: pipeline mutex busy");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"message": "Pipeline busy (room switch)."})),
                )
            })?;
        let mut warnings = pm.switch_room_live(&urls, geoms).map_err(|e| {
            tracing::error!("sim_switch_room: switch_room_live fehlgeschlagen: {e:?}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"message": format!("Raumwechsel fehlgeschlagen: {e}")})),
            )
        })?;

        // Audio-Gerät des Raums live setzen (nur wenn gesetzt; Hardwarewechsel kann kurz knacken)
        if let Some(dev) = room.audio_device.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            if let Err(e) = pm.set_audio_device(dev) {
                tracing::warn!(?e, "sim_switch_room: audio device switch failed");
                warnings.push(format!("Audio: {e}"));
            }
        }
        warnings
    };

    // 4) Raum-Viewer antriggern (falls konfiguriert und nicht schon aktiv)
    if let Some(viewer_id) = room.viewer_id.filter(|v| *v > 0) {
        let mut active_viewers: Vec<i64> = db::settings_get(&ctx.db, "sim.active_viewers")
            .await
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if !active_viewers.contains(&viewer_id) {
            let ws_path = format!("/api/webrtc/{}", session_id);
            crate::routes::send_viewer_command(&ctx, viewer_id, json!({"cmd": "reset_webrtc"})).await;
            crate::routes::send_viewer_command(
                &ctx,
                viewer_id,
                json!({"cmd": "show_live", "url": ws_path}),
            )
            .await;
            crate::routes::send_viewer_command(
                &ctx,
                viewer_id,
                json!({"action": "set_volume", "volume": 100}),
            )
            .await;
            active_viewers.push(viewer_id);
            if let Ok(list) = serde_json::to_string(&active_viewers) {
                let _ = db::settings_set(&ctx.db, "sim.active_viewers", &list).await;
            }
        }
    }

    // 5) Raum merken + Marker „Raumwechsel“ setzen (Typ „Hinweis“, sonst erster Typ)
    set_active_room_setting(&ctx, Some(room.id)).await;
    if let Ok(types) = db::marker_types_list(&ctx.db, None).await {
        let mt = types
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case("Hinweis"))
            .or_else(|| types.first());
        if let Some(mt) = mt {
            let text = format!("Raumwechsel → {}", room.name);
            match db::marker_add(&ctx.db, session_id, mt.id, user_id, Some(text.clone()), None).await {
                Ok(id) => {
                    let username = match user_id {
                        Some(uid) => db::user_by_id(&ctx.db, uid)
                            .await
                            .ok()
                            .flatten()
                            .map(|u| u.username),
                        None => None,
                    };
                    broadcast_ui(
                        &ctx,
                        json!({
                            "type": "marker_add",
                            "id": id,
                            "session_id": session_id,
                            "marker_type_id": mt.id,
                            "text": text,
                            "user_id": user_id,
                            "username": username.unwrap_or_else(|| "System".into()),
                        }),
                    )
                    .await;
                }
                Err(e) => tracing::warn!(?e, "sim_switch_room: marker_add failed"),
            }
        }
    }

    // 6) UI informieren
    broadcast_ui(
        &ctx,
        json!({
            "type": "room_switched",
            "session_id": session_id,
            "room_id": room.id,
            "room_name": room.name,
            "layout": room.layout,
            "previous_room_id": previous_room_id,
            "warnings": warnings,
        }),
    )
    .await;
    tracing::info!(
        session_id,
        room_id = room.id,
        warnings = warnings.len(),
        "sim_switch_room: done"
    );

    Ok(Json(SwitchRoomResponse {
        ok: true,
        session_id,
        room_id: room.id,
        room_name: room.name,
        layout: room.layout,
        warnings,
    }))
}

/// ------------------------------------------------------------
/// POST /api/sim/debrief/start
/// ------------------------------------------------------------
#[derive(Deserialize)]
#[allow(dead_code)]
pub struct DebriefStartPayload {
    pub session_id: Option<i64>,
}
pub async fn debrief_start(
    State(ctx): State<AppCtx>,
    headers: HeaderMap,
    Json(_p): Json<DebriefStartPayload>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    if let Some(uid) = crate::achievement::auth_user_id_from_headers(&ctx, &headers) {
        let username = crate::db::user_by_id(&ctx.db, uid)
            .await
            .ok()
            .flatten()
            .map(|u| u.username);
        match crate::achievement::on_debrief_started(&ctx.db, uid).await {
            Ok(keys) => {
                if !keys.is_empty() {
                    if let Ok(defs) = crate::achievement::achievement_payloads_by_keys(&ctx.db, &keys).await {
                        let msg = json!({
                            "type": "achievement_unlocked",
                            "user_id": uid,
                            "username": username,
                            "achievements": defs,
                        });
                        broadcast_ui(&ctx, msg).await;
                    }
                }
            }
            Err(e) => tracing::warn!(?e, user_id = uid, "achievement on_debrief_started failed"),
        }
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// ------------------------------------------------------------
/// POST /api/sim/debrief/position  (setzen)
/// ------------------------------------------------------------
pub async fn debrief_set_position(
    State(ctx): State<AppCtx>,
    Json(p): Json<DebriefPositionSet>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    db::settings_set(&ctx.db, "debrief.position_ms", &p.position_ms.to_string())
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// ------------------------------------------------------------
/// GET /api/sim/debrief/position  (holen)
/// ------------------------------------------------------------
pub async fn debrief_get_position(
    State(ctx): State<AppCtx>,
) -> Result<Json<DebriefPositionGet>, axum::http::StatusCode> {
    let pos = db::settings_get(&ctx.db, "debrief.position_ms")
        .await
        .ok()
        .flatten()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);

    let file = db::settings_get(&ctx.db, "debrief.vod_file")
        .await
        .ok()
        .flatten();

    let length_ms = if let Some(f) = &file {
        probe_duration_ms(&db::app_dirs().rec_dir.join("vod").join(f)).unwrap_or(600_000)
    } else {
        600_000
    };

    Ok(Json(DebriefPositionGet {
        position_ms: pos,
        length_ms,
        file,
    }))
}

/// ------------------------------------------------------------
/// GET /api/sim/last_finished  → letzte fertige Session
/// ------------------------------------------------------------
#[derive(Serialize)]
pub struct LastFinished {
    pub session_id: i64,
    pub file: Option<String>,
}
pub async fn last_finished(State(ctx): State<AppCtx>) -> Json<LastFinished> {
    let session_id = db::sim_last_finished_session_id(&ctx.db)
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
    let file = if session_id > 0 {
        db::sim_session_filename(&ctx.db, session_id)
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    if file.is_some() {
        let _ = db::settings_set(&ctx.db, "debrief.active", "true").await;
        let _ = db::settings_set(&ctx.db, "debrief.session_id", &session_id.to_string()).await;
        let _ = db::settings_set(&ctx.db, "debrief.vod_file", file.as_deref().unwrap()).await;
    }
    Json(LastFinished { session_id, file })
}

/// ------------------------------------------------------------
/// Helpers
/// ------------------------------------------------------------
async fn guess_vod_file(pool: &sqlx::SqlitePool, session_id: i64) -> Option<String> {
    // 1) Wenn in der DB schon etwas Sinnvolles steht → benutzen
    if let Ok(opt) = db::sim_session_filename(pool, session_id).await {
        if let Some(name) = opt {
            if !name.trim().is_empty() {
                return Some(name);
            }
        }
    }

    // 2) Fallback: Standard-Namen erzeugen
    Some(format!("session-{}.mkv", session_id))
}

/// sehr einfache ffprobe-Hilfe, ohne Netzabhängigkeit
pub(crate) fn probe_duration_ms(file: &PathBuf) -> Option<i64> {
    use std::process::Command;
    if !file.exists() {
        return None;
    }
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=nokey=1:noprint_wrappers=1",
        ])
        .arg(file)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let secs: f64 = s.parse().ok()?;
    Some((secs * 1000.0) as i64)
}

pub async fn sim_stop_all(
    State(ctx): State<AppCtx>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Pipeline einmal stoppen (falls sie läuft)
    stop_pipeline_soft(&ctx, "sim_stop_all").await;
    set_active_room_setting(&ctx, None).await;
    let cancelled_prestart = cancel_pending_prestart(&ctx, "Stopp (alle)").await;

    let mut stopped: Vec<i64> = Vec::new();

    // Variante A: Wenn du mehrere aktive Sessions zulässt,
    // stoppe sie nacheinander, bis keine mehr aktiv ist.
    loop {
        let sid_opt = db::sim_active_session_id(&ctx.db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let Some(sid) = sid_opt else {
            break; // nichts mehr aktiv
        };

        let now = Utc::now().timestamp();

        let start = db::sim_session_start_time(&ctx.db, sid)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::NOT_FOUND)?;

        let length = (now - start).max(0) as i64 * 1000;

        let dirs = db::app_dirs();
        let rec_dir = dirs.rec_dir.clone();
        let session_folder = rec_dir.join(sid.to_string());
        let vod_dir = rec_dir.join("vod");
        let _ = std::fs::create_dir_all(&vod_dir);

        // Dateiname der Session (für das spätere VOD)
        let vod_name = guess_vod_file(&ctx.db, sid)
            .await
            .unwrap_or_else(|| format!("session-{}.mkv", sid));

        // 👉 Echte Input-Datei suchen:
        //    1. Versuche session-<sid>_rec_*.mkv
        //    2. Fallback: session-<sid>.mkv
        use std::path::PathBuf;

        fn find_first_segment(session_folder: &PathBuf, sid: i64) -> PathBuf {
            let prefix = format!("session-{}_rec_", sid);

            if let Ok(rd) = std::fs::read_dir(session_folder) {
                for entry in rd.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with(&prefix) && name.ends_with(".mkv") {
                        return entry.path();
                    }
                }
            }

            // Fallback: eine einzige Datei ohne _rec_
            session_folder.join(format!("session-{}.mkv", sid))
        }

        let input_path = find_first_segment(&session_folder, sid);
        let input_path_str = input_path.to_string_lossy().to_string();
        let out_dir_str = vod_dir.to_string_lossy().to_string();

        // Job eintragen
        let _job_id = db::job_enqueue(&ctx.db, sid, &input_path_str, &out_dir_str, "mkv")
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        // Session finalisieren
        db::sim_finalize_session(&ctx.db, sid, now, length, Some(&vod_name))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        // Session finalisieren
        db::sim_finalize_session(&ctx.db, sid, now, length, Some(&vod_name))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        stopped.push(sid);
    }

    Ok(Json(json!({
        "ok": true,
        "stopped_sessions": stopped,
        "cancelled_prestart": cancelled_prestart
    })))
}
