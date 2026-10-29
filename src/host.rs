use crate::state::AppCtx;
use axum::{
    extract::{Path, Query, State},
    http::{
        header::{CONTENT_DISPOSITION, CONTENT_TYPE},
        HeaderMap, HeaderValue,
    },
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::env;
use std::net::IpAddr;
use std::process::Command;
use tokio::task::spawn_blocking;

use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};


#[derive(Serialize)]
pub struct HostStats {
    pub cpu_avg: f64,
    pub mem_total: u64,
    pub mem_used: u64,
    pub load_one: f64,
    pub load_five: f64,
    pub load_fifteen: f64,
    pub uptime_s: u64,
    pub disks: Vec<serde_json::Value>,
    pub net: Vec<serde_json::Value>,
    pub server_version: String,
    /// Verteiler-Ausgang (RTMP); von der Systemseite als `data.rtmp` gelesen.
    pub rtmp: crate::pipeline::DistributorStatus,
}

pub async fn stats_handler() -> Json<HostStats> {
    let mut sys = System::new_with_specifics(
        RefreshKind::new()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    );
    sys.refresh_memory();
    sys.refresh_cpu_specifics(CpuRefreshKind::everything());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    sys.refresh_cpu_specifics(CpuRefreshKind::everything());

    let cpus = sys.cpus();
    let cpu_avg = if cpus.is_empty() {
        0.0
    } else {
        cpus.iter().map(|c| c.cpu_usage() as f64).sum::<f64>() / (cpus.len() as f64)
    };
    let load = System::load_average();

    let disks_coll = Disks::new_with_refreshed_list();
    let disks: Vec<_> = disks_coll
        .iter()
        .map(|d| {
            serde_json::json!({
                "name": d.name().to_string_lossy(),
                "mount": d.mount_point().to_string_lossy(),
                "fs": d.file_system().to_string_lossy().to_string(),
                "total": d.total_space(),
                "available": d.available_space(),
            })
        })
        .collect();

    let nets_coll = Networks::new_with_refreshed_list();
    let net: Vec<_> = nets_coll.iter().map(|(name, data)| {
        serde_json::json!({ "name": name, "rx_bytes": data.received(), "tx_bytes": data.transmitted() })
    }).collect();

    Json(HostStats {
        cpu_avg,
        mem_total: sys.total_memory(),
        mem_used: sys.used_memory(),
        load_one: load.one,
        load_five: load.five,
        load_fifteen: load.fifteen,
        uptime_s: System::uptime(),
        disks,
        net,
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        rtmp: crate::pipeline::distributor_status(),
    })
}

#[derive(Serialize)]
pub struct HostIp {
    pub ip: String,
}

fn choose_non_loopback(ips: Vec<IpAddr>) -> Option<IpAddr> {
    for ip in ips {
        if !ip.is_loopback() {
            return Some(ip);
        }
    }
    None
}

pub async fn ip_handler() -> Json<HostIp> {
    let mut selected: Option<IpAddr> = None;
    if let Ok(ip) = local_ip_address::local_ip() {
        if !ip.is_loopback() {
            selected = Some(ip);
        }
    }
    if selected.is_none() {
        if let Ok(list) = local_ip_address::list_afinet_netifas() {
            let ips: Vec<IpAddr> = list.into_iter().map(|(_, ip)| ip).collect();
            selected = choose_non_loopback(ips);
        }
    }
    let ip = selected
        .unwrap_or_else(|| IpAddr::from([127, 0, 0, 1]))
        .to_string();
    Json(HostIp { ip })
}

// Sicherheits-Schalter host.control_enable in DB
async fn control_allowed(ctx: &AppCtx) -> bool {
    match crate::db::settings_get(&ctx.db, "host.control_enable").await {
        Ok(Some(v)) => v == "true",
        Ok(None) => true, // Default: erlaubt, wenn nicht gesetzt
        Err(e) => {
            tracing::warn!(
                ?e,
                "control_allowed: settings_get failed, allowing by default"
            );
            true
        }
    }
}

pub async fn reboot_handler(
    user: crate::routes::AuthUser,
    State(ctx): State<AppCtx>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    if user.role != "admin" {
        return Err(axum::http::StatusCode::FORBIDDEN);
    }
    if !control_allowed(&ctx).await {
        return Ok(Json(
            serde_json::json!({"ok": false, "error":"control disabled (set host.control_enable=true)"}),
        ));
    }
    let ok = spawn_blocking(|| crate::privileged::power("reboot")).await.unwrap_or(false);
    Ok(Json(serde_json::json!({ "ok": ok })))
}

pub async fn poweroff_handler(
    user: crate::routes::AuthUser,
    State(ctx): State<AppCtx>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    if user.role != "admin" {
        return Err(axum::http::StatusCode::FORBIDDEN);
    }
    if !control_allowed(&ctx).await {
        return Ok(Json(
            serde_json::json!({"ok": false, "error":"control disabled (set host.control_enable=true)"}),
        ));
    }
    let ok = spawn_blocking(|| crate::privileged::power("poweroff")).await.unwrap_or(false);
    Ok(Json(serde_json::json!({ "ok": ok })))
}

fn systemd_unit_name() -> String {
    std::env::var("ORS_SYSTEMD_UNIT")
        .or_else(|_| std::env::var("ORS_JOURNAL_UNIT"))
        .unwrap_or_else(|_| "openresquesim.service".to_string())
}

pub async fn restart_service_handler(
    user: crate::routes::AuthUser,
    State(ctx): State<AppCtx>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    if user.role != "admin" {
        return Err(axum::http::StatusCode::FORBIDDEN);
    }
    if !control_allowed(&ctx).await {
        return Ok(Json(
            serde_json::json!({"ok": false, "error":"control disabled (set host.control_enable=true)"}),
        ));
    }
    let unit = systemd_unit_name();
    if !crate::privileged::is_root() {
        // Ohne root: selbst beenden, systemd startet den Dienst neu (Restart=always).
        // Kurz warten, damit die Antwort noch beim Browser ankommt.
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            tracing::warn!("Dienst-Neustart aus der Oberfläche: beende Prozess, systemd startet neu");
            unsafe { libc::raise(libc::SIGTERM) };
        });
        return Ok(Json(serde_json::json!({ "ok": true, "unit": unit })));
    }
    let ok = spawn_blocking({
        let unit = unit.clone();
        move || Command::new("systemctl").arg("restart").arg(unit).status()
    })
    .await
    .ok()
    .and_then(|r| r.ok())
    .map(|st| st.success())
    .unwrap_or(false);
    Ok(Json(serde_json::json!({ "ok": ok, "unit": unit })))
}

#[derive(Serialize, Debug, Clone)]
pub struct AudioInputDevice {
    pub id: String,      // stabile ID, die du in audio.device speichern kannst
    pub name: String,    // schöner Name fürs UI
    pub backend: String, // z.B. "Audio/Source" oder Pulse/ALSA-Info
    /// Native Capture-Kanalzahl (None = unbekannt)
    pub channels: Option<u32>,
}

#[derive(Serialize)]
pub struct AudioInputDevicesResponse {
    pub devices: Vec<AudioInputDevice>,
}

// reine Sync-Funktion, die GStreamer fragt
fn list_audio_input_devices() -> Vec<AudioInputDevice> {
    // Nur ALSA-Karten anzeigen (robust, keine Pulse/PipeWire-Dubletten).
    list_alsa_cards()
}


/// Scannt ALSA-Capture-Devices via /proc/asound/cards (keine Abhängigkeit zu Pulse/PipeWire).
fn list_alsa_cards() -> Vec<AudioInputDevice> {
    let mut res = Vec::new();
    let Ok(txt) = std::fs::read_to_string("/proc/asound/cards") else {
        return res;
    };

    // Helfer: finde die niedrigste Capture-Subdevice-Nummer für eine Karte (pcmC<card>D<dev>c)
    fn first_capture_subdevice(card: u32) -> Option<u32> {
        let mut best: Option<u32> = None;
        if let Ok(dir) = std::fs::read_dir("/dev/snd") {
            for entry in dir.flatten() {
                if let Ok(fname) = entry.file_name().into_string() {
                    // Erwartet: pcmC{card}D{dev}c
                    if !fname.starts_with(&format!("pcmC{card}D")) || !fname.ends_with('c') {
                        continue;
                    }
                    // zwischen D und c liegt die Dev-Nummer
                    if let Some(dev_part) = fname
                        .trim_start_matches(&format!("pcmC{card}D"))
                        .strip_suffix('c')
                    {
                        if let Ok(dev_num) = dev_part.parse::<u32>() {
                            best = Some(best.map_or(dev_num, |b| b.min(dev_num)));
                        }
                    }
                }
            }
        }
        best
    }

    for line in txt.lines() {
        // Beispiel: " 0 [PCH            ]: HDA-Intel - HD-Audio Generic"
        let line = line.trim_start();
        let mut parts = line.splitn(2, ' ');
        let card_num_str = parts.next().unwrap_or_default();
        let rest = parts.next().unwrap_or_default();
        let card_num: u32 = card_num_str.parse().unwrap_or(0);

        // Nur Karten mit Capture-Device listen
        let Some(cap_dev) = first_capture_subdevice(card_num) else {
            continue;
        };

        // Name aus Klammern extrahieren, sonst rest nutzen
        let name = if let Some(start) = rest.find('[') {
            if let Some(end) = rest[start + 1..].find(']') {
                rest[start + 1..start + 1 + end].trim().to_string()
            } else {
                rest.trim().to_string()
            }
        } else {
            rest.trim().to_string()
        };

        // Wir bieten einen generischen Capture-Pfad an (plughw:<card>,<dev>) für automatische Format-Konvertierung.
        let id = format!("plughw:{card_num},{cap_dev}");
        let channels = crate::pipeline::detect_capture_channels(&id);
        res.push(AudioInputDevice {
            id,
            name: if name.is_empty() {
                format!("ALSA Card {card_num}")
            } else {
                name
            },
            backend: "ALSA".into(),
            channels,
        });
    }

    res
}


// HTTP-Handler: GET /api/host/audio_inputs
pub async fn audio_inputs_handler(State(_ctx): State<AppCtx>) -> Json<AudioInputDevicesResponse> {
    let devices = list_audio_input_devices();
    Json(AudioInputDevicesResponse { devices })
}

/// ------------------------
/// Journald-Logs anzeigen
/// ------------------------

#[derive(Deserialize, Default)]
#[allow(dead_code)]
pub struct LogsQuery {
    pub lines: Option<usize>,
    pub unit: Option<String>,
}

#[derive(Serialize)]
pub struct LogsResponse {
    pub ok: bool,
    pub unit: String,
    pub lines: Vec<String>,
    pub error: Option<String>,
}

pub async fn logs_handler(
    State(_ctx): State<AppCtx>,
    Query(q): Query<LogsQuery>,
) -> Json<LogsResponse> {
    let path = crate::logging::log_file_path();
    let unit_label = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "openresquesim.log".into());
    let max_lines = q.lines.unwrap_or(200).max(1).min(1000);

    match std::fs::read(&path) {
        Ok(buf) => {
            let lines: Vec<String> = String::from_utf8_lossy(&buf)
                .lines()
                .rev()
                .take(max_lines)
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            Json(LogsResponse {
                ok: true,
                unit: unit_label,
                lines,
                error: None,
            })
        }
        Err(e) => Json(LogsResponse {
            ok: false,
            unit: unit_label,
            lines: Vec::new(),
            error: Some(format!(
                "Logdatei nicht verfügbar: {e} ({})",
                path.display()
            )),
        }),
    }
}

#[derive(Serialize)]
pub struct LogFileEntry {
    pub name: String,
    pub size_bytes: u64,
    pub modified_at: Option<i64>,
}

#[derive(Serialize)]
pub struct LogFilesResponse {
    pub files: Vec<LogFileEntry>,
}

#[derive(Deserialize)]
pub struct LogCleanupPayload {
    pub keep_days: Option<u32>,
}

#[derive(Serialize)]
pub struct LogCleanupResponse {
    pub ok: bool,
    pub keep_days: u32,
    pub deleted_files: usize,
    pub deleted_bytes: u64,
    pub kept_files: usize,
}

pub async fn logs_files_handler(
    State(_ctx): State<AppCtx>,
    user: crate::routes::AuthUser,
) -> Result<Json<LogFilesResponse>, axum::http::StatusCode> {
    if user.role != "admin" {
        return Err(axum::http::StatusCode::FORBIDDEN);
    }
    let dir = crate::logging::log_dir_path();
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = match path.file_name().and_then(|s| s.to_str()) {
                Some(n) if n.ends_with(".log") => n.to_string(),
                _ => continue,
            };
            if let Ok(meta) = entry.metadata() {
                let modified_at = meta
                    .modified()
                    .ok()
                    .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64);
                files.push(LogFileEntry {
                    name,
                    size_bytes: meta.len(),
                    modified_at,
                });
            }
        }
    }
    files.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(Json(LogFilesResponse { files }))
}

pub async fn logs_file_download_handler(
    State(_ctx): State<AppCtx>,
    user: crate::routes::AuthUser,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, axum::http::StatusCode> {
    if user.role != "admin" {
        return Err(axum::http::StatusCode::FORBIDDEN);
    }
    if name.contains('/') || name.contains('\\') || !name.ends_with(".log") {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }

    let dir = crate::logging::log_dir_path();
    let path = dir.join(&name);
    let canonical_path =
        std::fs::canonicalize(&path).map_err(|_| axum::http::StatusCode::NOT_FOUND)?;
    let canonical_dir =
        std::fs::canonicalize(&dir).map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    if !canonical_path.starts_with(&canonical_dir) {
        return Err(axum::http::StatusCode::FORBIDDEN);
    }

    let bytes = tokio::fs::read(&canonical_path)
        .await
        .map_err(|_| axum::http::StatusCode::NOT_FOUND)?;
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    let disposition = format!("attachment; filename=\"{}\"", name);
    let disposition = HeaderValue::from_str(&disposition)
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    headers.insert(CONTENT_DISPOSITION, disposition);
    Ok((
        headers,
        bytes,
    ))
}

pub async fn logs_cleanup_handler(
    State(_ctx): State<AppCtx>,
    user: crate::routes::AuthUser,
    Json(body): Json<LogCleanupPayload>,
) -> Result<Json<LogCleanupResponse>, axum::http::StatusCode> {
    if user.role != "admin" {
        return Err(axum::http::StatusCode::FORBIDDEN);
    }

    let keep_days = body.keep_days.unwrap_or(7).min(3650);
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs((keep_days as u64) * 86_400))
        .ok_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let dir = crate::logging::log_dir_path();
    let active = crate::logging::log_file_path();
    let active_canonical = std::fs::canonicalize(active).ok();

    let mut deleted_files = 0usize;
    let mut deleted_bytes = 0u64;
    let mut kept_files = 0usize;

    if let Ok(rd) = std::fs::read_dir(&dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name_ok = path
                .file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.ends_with(".log"))
                .unwrap_or(false);
            if !name_ok {
                continue;
            }
            let canonical = std::fs::canonicalize(&path).ok();
            if canonical.is_some() && canonical == active_canonical {
                kept_files += 1;
                continue;
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let modified = match meta.modified() {
                Ok(m) => m,
                Err(_) => {
                    kept_files += 1;
                    continue;
                }
            };
            if modified > cutoff {
                kept_files += 1;
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                deleted_files += 1;
                deleted_bytes = deleted_bytes.saturating_add(meta.len());
            } else {
                kept_files += 1;
            }
        }
    }

    Ok(Json(LogCleanupResponse {
        ok: true,
        keep_days,
        deleted_files,
        deleted_bytes,
        kept_files,
    }))
}
