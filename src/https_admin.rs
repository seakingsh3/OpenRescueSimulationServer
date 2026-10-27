//! HTTPS aus der Oberfläche einrichten/erneuern (Systemseite → „🔒 HTTPS“, nur Admin).
//!
//! Führt `scripts/setup_https.sh` aus. Das Skript braucht root und startet am Ende den
//! Dienst neu; deshalb läuft es als eigene, kurzlebige systemd-Unit (`systemd-run`),
//! außerhalb des Dienstes – der Neustart reißt es so nicht mit. Die Ausgabe landet in
//! einer Logdatei, die die Oberfläche anzeigt.
//!
//! Rechte: Läuft der Dienst als root, startet er `systemd-run` direkt. Sonst über
//! `sudo -n` mit der Regel aus `/etc/sudoers.d/openresquesim` (liegt im Debian-Paket und
//! erlaubt genau diesen Aufruf). Fehlt beides, meldet die Oberfläche das.
//! Es gibt keine freien Befehle: Nur Hostnamen/IP-Adressen werden als Argumente übergeben.

use std::path::{Path, PathBuf};
use std::process::Command;

use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::routes::AuthUser;
use crate::state::AppCtx;

const UNIT: &str = "ors-https-setup";
const LOG_FILE: &str = "/var/log/openresquesim-https-setup.log";
const DEFAULT_CERT: &str = "/etc/openresquesim/tls/server.pem";

fn require_admin(user: &AuthUser) -> Result<(), StatusCode> {
    if user.role == "admin" { Ok(()) } else { Err(StatusCode::FORBIDDEN) }
}

/// Testbetrieb: Ersatzskript direkt ausführen (ohne root/systemd), Log im Datenordner.
fn test_script() -> Option<PathBuf> {
    std::env::var("ORS_HTTPS_SETUP_SCRIPT").ok().filter(|s| !s.trim().is_empty()).map(PathBuf::from)
}

fn log_path() -> PathBuf {
    if test_script().is_some() {
        crate::db::app_dirs().base_dir.join("https-setup.log")
    } else {
        PathBuf::from(LOG_FILE)
    }
}

fn script_path() -> Option<PathBuf> {
    if let Some(p) = test_script() {
        return Some(p);
    }
    ["/usr/share/openresquesim/scripts/setup_https.sh", "scripts/setup_https.sh"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// Nur Hostnamen (Buchstaben, Ziffern, Bindestrich, Punkt) und IP-Adressen zulassen
fn valid_name(n: &str) -> bool {
    if n.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    !n.is_empty()
        && n.len() <= 253
        && !n.starts_with('-')
        && n.split('.').all(|l| {
            !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

fn unit_active() -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", UNIT])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

static TEST_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn running() -> bool {
    if test_script().is_some() {
        TEST_RUNNING.load(std::sync::atomic::Ordering::Relaxed)
    } else {
        unit_active()
    }
}

/// Zertifikat auslesen (openssl): gültig bis, Namen
fn cert_info(path: &Path) -> Value {
    if !path.is_file() {
        return json!(null);
    }
    let out = Command::new("openssl")
        .args(["x509", "-noout", "-enddate", "-ext", "subjectAltName", "-in"])
        .arg(path)
        .output();
    let Ok(out) = out else { return json!({ "path": path, "error": "openssl fehlt" }) };
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let not_after = text
        .lines()
        .find_map(|l| l.strip_prefix("notAfter="))
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let names: Vec<String> = text
        .lines()
        .filter(|l| l.contains("DNS:") || l.contains("IP Address:"))
        .flat_map(|l| l.split(','))
        .map(|p| p.trim().trim_start_matches("DNS:").trim_start_matches("IP Address:").to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let expires = chrono::NaiveDateTime::parse_from_str(&not_after.replace(" GMT", ""), "%b %e %H:%M:%S %Y")
        .ok()
        .map(|d| d.and_utc().timestamp());
    json!({ "path": path, "not_after": not_after, "expires": expires, "names": names })
}

pub async fn https_status(user: AuthUser, State(_ctx): State<AppCtx>) -> Result<Json<Value>, StatusCode> {
    require_admin(&user)?;
    let cert_path = std::env::var("ORS_TLS_CERT").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(DEFAULT_CERT));
    let tls_active = std::env::var("ORS_TLS_CERT").is_ok() && cert_path.is_file();
    let tls_bind = std::env::var("ORS_TLS_BIND").unwrap_or_else(|_| "0.0.0.0:8443".into());
    let log = std::fs::read_to_string(log_path()).unwrap_or_default();
    let log_tail: String = log.lines().rev().take(200).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    let root_ca = root_ca_path().is_some();
    let can_run = script_path().is_some() && (test_script().is_some() || is_root() || Path::new("/etc/sudoers.d/openresquesim").exists());
    Ok(Json(json!({
        "tls_active": tls_active,
        "tls_port": tls_bind.rsplit(':').next().unwrap_or("8443"),
        "cert": cert_info(&cert_path),
        "running": running(),
        "log": log_tail,
        "root_ca_download": root_ca.then_some("/ca/ors-rootCA.crt"),
        "can_run": can_run,
        "reason": if script_path().is_none() { Some("Skript setup_https.sh nicht gefunden") }
                  else if !can_run { Some("Server läuft nicht als root und es gibt keine sudo-Regel (/etc/sudoers.d/openresquesim)") }
                  else { None },
    })))
}

#[derive(Deserialize, Default)]
pub struct SetupPayload {
    #[serde(default)]
    pub names: Vec<String>,
}

pub async fn https_setup(
    user: AuthUser,
    State(_ctx): State<AppCtx>,
    Json(body): Json<SetupPayload>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_admin(&user).map_err(|s| (s, Json(json!({"error": "nur für Admins"}))))?;
    let names: Vec<String> = body.names.iter().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect();
    if let Some(bad) = names.iter().find(|n| !valid_name(n)) {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": format!("Ungültiger Name/IP: {bad}")}))));
    }
    if names.len() > 20 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "höchstens 20 Namen/IPs"}))));
    }
    if running() {
        return Err((StatusCode::CONFLICT, Json(json!({"error": "Einrichtung läuft bereits"}))));
    }
    let Some(script) = script_path() else {
        return Err((StatusCode::NOT_FOUND, Json(json!({"error": "Skript setup_https.sh nicht gefunden"}))));
    };
    let log = log_path();
    let _ = std::fs::write(&log, format!("Gestartet von {} …\n", user.username));
    tracing::warn!(by = %user.username, ?names, "🔒 HTTPS-Einrichtung aus der Oberfläche gestartet");

    // Testbetrieb: Ersatzskript direkt, Ausgabe in die Logdatei
    if test_script().is_some() {
        TEST_RUNNING.store(true, std::sync::atomic::Ordering::Relaxed);
        let file = std::fs::OpenOptions::new().append(true).open(&log).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
        let err = file.try_clone().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
        let mut child = Command::new(&script).args(&names).stdout(file).stderr(err).spawn()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
        std::thread::spawn(move || {
            let _ = child.wait();
            TEST_RUNNING.store(false, std::sync::atomic::Ordering::Relaxed);
        });
        return Ok(Json(json!({"ok": true})));
    }

    // Eigene systemd-Unit, damit der Dienst-Neustart am Ende das Skript nicht beendet
    let log_arg = format!("append:{}", log.display());
    let mut args: Vec<String> = vec![
        "--unit".into(), UNIT.into(), "--collect".into(), "--quiet".into(),
        "-p".into(), format!("StandardOutput={log_arg}"),
        "-p".into(), format!("StandardError={log_arg}"),
        script.display().to_string(),
    ];
    args.extend(names);
    let status = if is_root() {
        Command::new("systemd-run").args(&args).status()
    } else {
        // sudo -n: nie nach einem Passwort fragen; erlaubt ist nur dieser Aufruf (sudoers-Regel)
        Command::new("sudo").arg("-n").arg("systemd-run").args(&args).status()
    };
    match status {
        Ok(s) if s.success() => Ok(Json(json!({"ok": true}))),
        Ok(s) => {
            let msg = if is_root() { format!("systemd-run fehlgeschlagen ({s})") } else {
                "Keine Rechte: Der Server läuft nicht als root, und sudo erlaubt den Aufruf nicht (Regel /etc/sudoers.d/openresquesim)".into()
            };
            let _ = std::fs::write(&log, format!("{msg}\n"));
            Err((StatusCode::FORBIDDEN, Json(json!({"error": msg}))))
        }
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))),
    }
}

/// Wurzelzertifikat der lokalen Zertifizierungsstelle (zum Installieren auf iPads/PCs)
fn root_ca_path() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = vec![
        PathBuf::from("/etc/openresquesim/tls/rootCA.pem"),
        PathBuf::from("/usr/share/openresquesim/web/frontend/static/ca/ors-rootCA.crt"),
        PathBuf::from("src/web/frontend/static/ca/ors-rootCA.crt"),
    ];
    if test_script().is_some() {
        candidates.insert(0, crate::db::app_dirs().base_dir.join("rootCA.pem"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// GET /ca/ors-rootCA.crt – öffentlich: Ein Wurzelzertifikat ist nicht geheim, und Geräte
/// sollen es laden können, bevor jemand angemeldet ist. Der Dateityp
/// application/x-x509-ca-cert lässt iPads es direkt als Profil installieren.
pub async fn root_ca_download() -> axum::response::Response {
    use axum::response::IntoResponse;
    match root_ca_path().and_then(|p| std::fs::read(p).ok()) {
        Some(bytes) => (
            [
                ("Content-Type", "application/x-x509-ca-cert"),
                ("Content-Disposition", "attachment; filename=\"ors-rootCA.crt\""),
                ("Cache-Control", "no-store"),
            ],
            bytes,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "Noch kein Wurzelzertifikat – HTTPS zuerst einrichten.").into_response(),
    }
}
