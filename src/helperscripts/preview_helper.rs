use anyhow::{anyhow, Context, Result};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::fs;
use tokio::sync::Mutex;
use tokio::task::spawn_blocking;

// Minimal Rate-Limiter pro Kamera, damit wir ffmpeg nicht spammen.
static SNAPSHOT_RATE: Lazy<Mutex<HashMap<i64, Instant>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Speichert Snapshots in einem stabilen Pfad unterhalb der App-Daten.
pub fn snapshot_path_base() -> PathBuf {
    let dirs = crate::db::app_dirs();
    dirs.data_dir.join("camera_previews")
}

/// Liefert den erwarteten Snapshot-Pfad für eine Kamera-ID.
pub fn snapshot_path_for(camera_id: i64) -> PathBuf {
    snapshot_path_base().join(format!("{camera_id}.jpg"))
}

/// Fügt eine weiche Rate-Limitierung hinzu (default 5 Sekunden).
async fn allow_new_snapshot(camera_id: i64, min_interval: Duration) -> bool {
    let mut map = SNAPSHOT_RATE.lock().await;
    let now = Instant::now();
    let entry = map
        .entry(camera_id)
        .or_insert(Instant::now() - min_interval);
    if now.duration_since(*entry) >= min_interval {
        *entry = now;
        true
    } else {
        false
    }
}

/// Ruft ffmpeg auf, um genau ein Frame zu ziehen, und schreibt es in `out_path`.
fn build_transport_order() -> Vec<String> {
    // Override via ENV: ORS_PREVIEW_RTSP_TRANSPORT=tcp|udp
    if let Ok(v) = std::env::var("ORS_PREVIEW_RTSP_TRANSPORT") {
        let t = v.trim().to_lowercase();
        if t == "tcp" || t == "udp" {
            return vec![t];
        }
    }
    vec!["tcp".into(), "udp".into()]
}

async fn pull_with_ffmpeg(url: &str, out_path: &Path) -> Result<()> {
    if let Some(dir) = out_path.parent() {
        fs::create_dir_all(dir).await.ok();
    }

    let url = url.to_string();
    let out = out_path.to_path_buf();

    // Für RTSP mehrere Transports probieren (tcp -> udp), sonst nur einmal.
    let transports: Vec<String> = if url.starts_with("rtsp://") || url.starts_with("rtsps://") {
        build_transport_order()
    } else {
        vec![]
    };

    let mut last_err: Option<anyhow::Error> = None;

    // Hilfsclosure für einen einzelnen Versuch
    let try_once = |transport: Option<&str>| {
        let url = url.clone();
        let out = out.clone();
        let transport = transport.map(|s| s.to_string());
        spawn_blocking(move || {
            let mut cmd = std::process::Command::new("ffmpeg");
            cmd.arg("-y").arg("-nostdin").arg("-loglevel").arg("error");

            // Stabiler Transport für RTSP, optional überschreibbar
            if url.starts_with("rtsp://") || url.starts_with("rtsps://") {
                let t = transport.as_deref().unwrap_or("tcp");
                cmd.arg("-rtsp_transport").arg(t);
                // Manche ffmpeg-Builds kennen weder stimeout noch rw_timeout zuverlässig.
                // Wir verzichten hier auf Timeout-Flags, um Kompatibilität zu wahren.
            }

            cmd.arg("-i")
                .arg(&url)
                .arg("-frames:v")
                .arg("1")
                .arg("-q:v")
                .arg("2")
                .arg(&out);

            cmd.output()
        })
    };

    if transports.is_empty() {
        let outp = try_once(None)
            .await
            .context("spawn ffmpeg for snapshot")??;
        if outp.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&outp.stderr);
        return Err(anyhow!("ffmpeg exited with {} ({stderr})", outp.status));
    }

    for t in transports {
        let outp = try_once(Some(&t))
            .await
            .context("spawn ffmpeg for snapshot")??;
        if outp.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&outp.stderr);
        last_err = Some(anyhow!(
            "ffmpeg transport={t} exited with {} ({stderr})",
            outp.status
        ));
    }

    Err(last_err.unwrap_or_else(|| anyhow!("ffmpeg snapshot failed")))
}

/// Liefert den Snapshot als Bytes. Holt bei Bedarf (oder wenn `force`) einen neuen.
pub async fn get_or_create_snapshot(
    db: &sqlx::SqlitePool,
    camera_id: i64,
    force: bool,
) -> Result<Vec<u8>> {
    let path = snapshot_path_for(camera_id);

    if !force {
        if let Ok(bytes) = fs::read(&path).await {
            return Ok(bytes);
        }
    }

    // Rate-Limit: wenn nicht force, respektiere Mindestintervall.
    if !force && !allow_new_snapshot(camera_id, Duration::from_secs(5)).await {
        if let Ok(bytes) = fs::read(&path).await {
            return Ok(bytes);
        }
    }

    let cam = crate::db::camera_get(db, camera_id)
        .await
        .context("load camera for snapshot")?;

    pull_with_ffmpeg(&cam.url, &path)
        .await
        .context("ffmpeg snapshot failed")?;

    let bytes = fs::read(&path)
        .await
        .context("read snapshot after ffmpeg")?;

    Ok(bytes)
}
