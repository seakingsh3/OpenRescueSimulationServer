use crate::db;
use crate::sim;
use crate::state::AppCtx; // oder wie dein Kontext-Modul heißt

use anyhow::Result;
use sqlx::SqlitePool;
use std::process::Command;

use axum::http::StatusCode;
use axum::{
    extract::{Path, State},
    Json,
};

pub async fn run_transcode_worker(pool: SqlitePool) -> Result<()> {
    loop {
        // Nächsten pending Job holen
        if let Some(job) = db::job_next_pending(&pool).await? {
            db::job_mark_running(&pool, job.id).await?;

            let input = &job.input_path;
            let out_dir = std::path::Path::new(&job.out_dir);
            std::fs::create_dir_all(out_dir).ok();

            // Dateiname aus Session ziehen – Endung auf .mp4 drehen
            let mut filename = db::sim_session_filename(&pool, job.session_id)
                .await?
                .unwrap_or_else(|| format!("session-{}.mp4", job.session_id));

            if !filename.to_lowercase().ends_with(".mp4") {
                // alte .mkv-Endung o.ä. ersetzen
                if let Some(stripped) = filename.strip_suffix(".mkv") {
                    filename = format!("{}.mp4", stripped);
                } else {
                    filename = format!("{}.mp4", filename);
                }
            }

            let out_path = out_dir.join(&filename);

            let mut cmd = Command::new("ffmpeg");
            cmd.arg("-y");

            // Wenn splitmuxsink mehrere Segmente erzeugt hat (session-XXX_rec_%05d.mkv),
            // bauen wir eine concat-Liste, damit ffmpeg alle Segmente nimmt.
            let mut concat_list: Option<std::path::PathBuf> = None;
            let mut prefix = String::new();
            let mut suffix = String::new();
            let p = std::path::Path::new(input);
            let parent = p
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf();
            let fname = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            if input.contains('%') {
                if let Some(pos) = fname.find('%') {
                    prefix = fname[..pos].to_string();
                    suffix = fname
                        .rsplit_once('.')
                        .map(|(_, ext)| format!(".{ext}"))
                        .unwrap_or_else(|| "".into());
                }
            } else {
                // Beispiel: session-340_rec_00000.mkv -> prefix=session-340_rec_, suffix=.mkv
                if let Some((base, ext)) = fname.rsplit_once('.') {
                    if let Some(idx) = base.rfind('_') {
                        let tail = &base[idx + 1..];
                        if tail.chars().all(|c| c.is_ascii_digit()) && tail.len() >= 5 {
                            prefix = format!("{}_", &base[..idx]);
                            suffix = format!(".{}", ext);
                        }
                    }
                }
            }

            if !prefix.is_empty() {
                // Alle passenden Segmente einsammeln und sortieren
                let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&parent)
                    .ok()
                    .into_iter()
                    .flat_map(|iter| iter.filter_map(|e| e.ok()))
                    .map(|e| e.path())
                    .filter(|p| {
                        if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                            name.starts_with(&prefix) && name.ends_with(&suffix)
                        } else {
                            false
                        }
                    })
                    .collect();
                files.sort(); // Dateinamen enthalten die Nummerierung → lexikographisch ok

                if !files.is_empty() {
                    tracing::info!(
                        "VOD concat: {} Segmente gefunden für {}",
                        files.len(),
                        prefix
                    );
                    let list_path = parent.join(format!("concat_job{}_list.txt", job.id));
                    if let Ok(mut f) = std::fs::File::create(&list_path) {
                        use std::io::Write;
                        for fp in &files {
                            // ffmpeg concat: file '<path>'
                            let escaped = fp.display().to_string().replace('\'', "'\\''");
                            let _ = writeln!(f, "file '{}'", escaped);
                        }
                        concat_list = Some(list_path.clone());
                        cmd.args(["-f", "concat", "-safe", "0", "-i"]);
                        cmd.arg(list_path);
                    }
                } else {
                    tracing::warn!(
                        "VOD concat: keine Segmente für Prefix '{}' in {:?} gefunden, fallback auf Einzel-Input",
                        prefix,
                        parent
                    );
                }
            }

            // Fallback: einzelnes File
            if concat_list.is_none() {
                cmd.arg("-i").arg(input);
            }

            match job.mode.as_str() {
                "mp4" => {
                    cmd.args([
                        "-c:v",
                        "copy", // Video kopieren (H.264)
                        "-c:a",
                        "aac", // Audio transcodieren, damit Browser es abspielen
                        "-b:a",
                        "160k",
                        "-ac",
                        "2",
                        "-movflags",
                        "+faststart",
                    ]);
                }
                // falls du irgendwann mal Lossless-VOD willst:
                "copy" | "mkv" => {
                    cmd.args(["-c", "copy"]);
                }
                _ => {
                    cmd.args(["-c", "copy"]);
                }
            }

            cmd.arg(&out_path);

            // Optional: ffmpeg-Logs stark drosseln, wenn nicht explizit gewünscht
            let verbose = std::env::var("FFMPEG_VERBOSE")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            if !verbose {
                cmd.args(["-hide_banner", "-loglevel", "warning"]);
            }

            tracing::info!(
                "VOD worker: starting ffmpeg (verbose={}) {:?}",
                verbose,
                cmd
            );

            let output = cmd.output();

            match output {
                Ok(out) if out.status.success() => {
                    // Dauer aus ffprobe ermitteln
                    let duration_sec = sim::probe_duration_ms(&out_path.to_path_buf())
                        .map(|ms| ms as f64 / 1000.0);

                    let size_bytes = std::fs::metadata(&out_path).ok().map(|m| m.len() as i64);

                    // vod_items-Eintrag anlegen
                    let rel_url = format!("/vod/{}", filename);
                    db::vod_add(
                        &pool,
                        job.session_id,
                        "mp4",
                        Some("default"),
                        out_path.to_str(),
                        Some(&rel_url),
                        size_bytes,
                        duration_sec,
                    )
                    .await?;

                    db::job_finish_ok(&pool, job.id, None).await?;
                }
                Ok(out) => {
                    let log = String::from_utf8_lossy(&out.stderr).to_string();
                    eprintln!("VOD worker: ffmpeg failed for job {}: {}", job.id, log);
                    db::job_finish_err(&pool, job.id, &log).await?;
                }
                Err(e) => {
                    eprintln!(
                        "VOD worker: failed to spawn ffmpeg for job {}: {}",
                        job.id, e
                    );
                    db::job_finish_err(&pool, job.id, &format!("spawn failed: {}", e)).await?;
                }
            }

            // Aufräumen: concat-Liste löschen, falls erzeugt
            if let Some(list) = concat_list {
                let _ = std::fs::remove_file(list);
            }
        } else {
            // Nichts zu tun → kurz schlafen
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }
}

#[derive(serde::Serialize)]
pub struct VodListResponse {
    pub items: Vec<db::VodItem>,
}

pub async fn vod_list_for_session(
    State(ctx): State<AppCtx>,
    Path(session_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> Result<Json<VodListResponse>, StatusCode> {
    crate::routes::ensure_session_visible(&ctx, &headers, session_id).await?;
    let items = db::vod_list_by_session(&ctx.db, session_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(VodListResponse { items }))
}
