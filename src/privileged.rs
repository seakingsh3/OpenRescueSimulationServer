//! Aufrufe, die root brauchen (HTTPS einrichten, Update installieren, Neustart/Herunterfahren).
//!
//! Der Dienst läuft als Benutzer `openresquesim`. Die erlaubten Aufrufe stehen einzeln in
//! `/etc/sudoers.d/openresquesim` (scripts/sudoers.openresquesim); die Befehlszeilen hier müssen
//! genau dazu passen. Läuft der Server doch als root (alte Installation), ohne sudo.
//!
//! Längere Aufgaben laufen als eigene, kurzlebige systemd-Unit: Sie überstehen so den
//! Neustart des Dienstes, den sie am Ende auslösen.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Logordner des Dienstes (gehört dem Dienstbenutzer; systemd hängt als root an)
pub const LOG_DIR: &str = "/var/log/openresquesim";

pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn log_path(name: &str) -> PathBuf {
    Path::new(LOG_DIR).join(name)
}

fn with_sudo(args: &[String]) -> Command {
    if is_root() {
        let mut c = Command::new(&args[0]);
        c.args(&args[1..]);
        c
    } else {
        // -n: nie nach einem Passwort fragen
        let mut c = Command::new("sudo");
        c.arg("-n").args(args);
        c
    }
}

fn unit_command(unit: &str, log: &Path, script: &Path, script_args: &[String]) -> Vec<String> {
    let log_arg = format!("append:{}", log.display());
    let mut args: Vec<String> = vec![
        "/usr/bin/systemd-run".into(), "--unit".into(), unit.into(), "--collect".into(), "--quiet".into(),
        "-p".into(), format!("StandardOutput={log_arg}"),
        "-p".into(), format!("StandardError={log_arg}"),
        script.display().to_string(),
    ];
    args.extend(script_args.iter().cloned());
    args
}

/// Skript als eigene systemd-Unit starten; Ausgabe in `log`
pub fn start_unit(unit: &str, log: &Path, script: &Path, script_args: &[String]) -> Result<(), String> {
    let args = unit_command(unit, log, script, script_args);
    match with_sudo(&args).status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) if is_root() => Err(format!("systemd-run fehlgeschlagen ({s})")),
        Ok(_) => Err("Keine Rechte: sudo erlaubt den Aufruf nicht (Regel /etc/sudoers.d/openresquesim fehlt oder passt nicht)".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Darf der Dienst die Unit starten? (root, oder sudo-Regel passt). Ergebnis je Unit
/// zwischengespeichert: sudoers ändert sich nur mit dem Paket, und das startet den Dienst neu.
pub fn can_start_unit(unit: &str, log: &Path, script: &Path) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    if is_root() {
        return true;
    }
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap().get(unit) {
        return *v;
    }
    let mut args = unit_command(unit, log, script, &[]);
    args.push("probe".into());
    let allowed = Command::new("sudo")
        .args(["-n", "-l"])
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    cache.lock().unwrap().insert(unit.to_string(), allowed);
    allowed
}

pub fn unit_active(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", unit])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `systemctl reboot` bzw. `poweroff`
pub fn power(action: &str) -> bool {
    let args = vec!["/usr/bin/systemctl".to_string(), action.to_string()];
    with_sudo(&args).status().map(|s| s.success()).unwrap_or(false)
}

/// Logdatei mit einer ersten Zeile neu beginnen (Ordner anlegen, falls nötig)
pub fn start_log(log: &Path, line: &str) {
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(log, format!("{line}\n"));
}
