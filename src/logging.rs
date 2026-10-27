// src/logging.rs
use axum::extract::ws::{Message, WebSocket};
use chrono::Local;
use gst::glib;
use gstreamer as gst;
use gstreamer::prelude::*;
use std::env;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::runtime::Handle as TokioHandle;
use tokio::select;
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio::time::Interval;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::{fmt, EnvFilter};

// WICHTIG: BusWatchGuard importieren (aus dem gstreamer-Crate-Namespace)

// RAII-Guard: solange er lebt, ist der Bus-Watch aktiv.
// (BusWatchGuard kümmert sich beim Drop selbst ums Entfernen.)
// Einfacher Typalias: wir behalten den echten Guard bei uns
pub type GstBusGuard = gstreamer::bus::BusWatchGuard;

// Bus anhängen wie gehabt (add_watch oder add_watch_local – egal)

pub fn attach_gst_bus(p: &gst::Pipeline) -> Option<GstBusGuard> {
    let bus = p.bus()?;
    let pipeline_weak = p.downgrade();
    let mut last_rtsp_reset = Instant::now() - Duration::from_secs(10);
    let mut last_record_alert = Instant::now() - Duration::from_secs(10);

    bus.add_watch(move |_, msg| {
        use gst::MessageView;

        // Für automatische RTSP-Reconnects bei Fehlern/EOS
        let (is_rtsp_src, rtsp_name) = msg
            .src()
            .map(|s| {
                let name = s.name();
                (name.starts_with("rtsp_"), name)
            })
            .unwrap_or((false, String::new().into()));

        match msg.view() {
            MessageView::Error(e) => {
                let err = e.error();
                let dbg = e.debug();
                tracing::error!(%err, ?dbg, "GST error");
                let src_name = msg
                    .src()
                    .map(|s| s.name().to_string())
                    .unwrap_or_default();
                let src_lc = src_name.to_ascii_lowercase();
                let err_lc = err.to_string().to_ascii_lowercase();
                let dbg_raw = dbg
                    .clone()
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                let dbg_lc = dbg_raw.to_ascii_lowercase();
                let is_record_branch = src_lc.starts_with("rec_")
                    || src_lc.contains("splitmux")
                    || dbg_lc.contains("splitmux")
                    || dbg_lc.contains("rec_")
                    || (src_lc == "enc" && err_lc.contains("encode x264 frame failed"));

                if is_record_branch && last_record_alert.elapsed() > Duration::from_secs(5) {
                    last_record_alert = Instant::now();
                    let short_dbg = if dbg_raw.len() > 260 {
                        format!("{}…", &dbg_raw[..260])
                    } else {
                        dbg_raw
                    };
                    emit_ui_event(serde_json::json!({
                        "type": "record_failed",
                        "message": err.to_string(),
                        "element": src_name,
                        "details": short_dbg,
                        "ts": chrono::Utc::now().to_rfc3339(),
                    }));
                }

                if is_rtsp_src && crate::pipeline::is_being_torn_down(&rtsp_name) {
                    // Element wird gerade per Live-Quellentausch abgebaut: Fehler erwartbar, kein Reset.
                    tracing::debug!("RTSP error auf {rtsp_name} während Teardown ignoriert");
                } else if is_rtsp_src {
                    // Nur den betroffenen RTSP-Source-Element-State toggeln, damit der Rest der Pipeline (Recording/WebRTC) nicht abgewürgt wird.
                    if last_rtsp_reset.elapsed() > Duration::from_secs(5) {
                        last_rtsp_reset = Instant::now();
                        if let Some(p) = pipeline_weak.upgrade() {
                            let name = rtsp_name.clone();
                            tracing::warn!(
                                "RTSP error auf {name} -> versuche Source-Reset (NULL→PLAYING)"
                            );
                            glib::MainContext::default().invoke(move || {
                                if crate::pipeline::is_being_torn_down(&name) {
                                    return;
                                }
                                if let Some(el) = p.by_name(&name) {
                                    let _ = el.set_state(gst::State::Null);
                                    let _ = el.set_state(gst::State::Playing);
                                } else {
                                    // Element existiert nicht mehr (z.B. nach Live-Quellentausch):
                                    // kein Pipeline-Reset, der würde Aufnahme und Viewer unterbrechen.
                                    tracing::warn!(
                                        "RTSP-Element {name} nicht mehr vorhanden, kein Reset"
                                    );
                                }
                            });
                        }
                    } else {
                        tracing::warn!("RTSP error (throttled, kein Reset <5s)");
                    }
                }
            }
            MessageView::Warning(w) => {
                let err = w.error();
                let dbg = w.debug();
                tracing::warn!(%err, ?dbg, "GST warn");
            }
            MessageView::Eos(_) => {
                tracing::info!("GST EOS");
                if is_rtsp_src {
                    if let Some(p) = pipeline_weak.upgrade() {
                        tracing::warn!("RTSP EOS -> versuche Pipeline-Reset (Ready→Playing)");
                        glib::MainContext::default().invoke(move || {
                            let _ = p.set_state(gst::State::Ready);
                            let _ = p.set_state(gst::State::Playing);
                        });
                    }
                }
            }
            _ => {}
        }

        // Statt glib::Continue(true):
        glib::ControlFlow::Continue
    })
    .ok()
}

// Guard im GLib-Thread droppen (gegen den "Value dropped on a different thread"-Panic)
pub fn detach_gst_bus(guard_opt: &mut Option<GstBusGuard>) {
    if let Some(guard) = guard_opt.take() {
        glib::MainContext::default().invoke(move || drop(guard));
    }
}

static LOG_FILE_PATH: OnceLock<PathBuf> = OnceLock::new();
static LOG_DIR_PATH: OnceLock<PathBuf> = OnceLock::new();
static UI_BROADCAST: OnceLock<Arc<AsyncMutex<Vec<mpsc::UnboundedSender<String>>>>> =
    OnceLock::new();
static TOKIO_HANDLE: OnceLock<TokioHandle> = OnceLock::new();

pub fn set_ui_broadcast(channels: Arc<AsyncMutex<Vec<mpsc::UnboundedSender<String>>>>) {
    let _ = UI_BROADCAST.set(channels);
}

pub fn set_tokio_handle(handle: TokioHandle) {
    let _ = TOKIO_HANDLE.set(handle);
}

pub(crate) fn emit_ui_event(value: serde_json::Value) {
    let Some(channels) = UI_BROADCAST.get().cloned() else {
        return;
    };
    let Some(handle) = TOKIO_HANDLE.get().cloned() else {
        return;
    };
    let msg = value.to_string();
    handle.spawn(async move {
        let mut chans = channels.lock().await;
        chans.retain(|tx| tx.send(msg.clone()).is_ok());
    });
}

fn resolve_log_dir() -> PathBuf {
    if let Ok(dir) = env::var("ORS_LOG_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }

    let candidates = [
        PathBuf::from("/var/log/openresquesim"),
        crate::db::app_dirs().base_dir.join("logs"),
        env::temp_dir().join("openresquesim"),
    ];

    for dir in candidates {
        if std::fs::create_dir_all(&dir).is_ok() {
            return dir;
        }
    }

    env::temp_dir()
}

pub fn log_file_path() -> PathBuf {
    if let Some(p) = LOG_FILE_PATH.get().cloned() {
        return p;
    }
    let dir = LOG_DIR_PATH
        .get()
        .cloned()
        .unwrap_or_else(resolve_log_dir);
    dir.join(today_log_filename())
}

pub fn log_dir_path() -> PathBuf {
    if let Some(dir) = LOG_DIR_PATH.get().cloned() {
        return dir;
    }
    log_file_path()
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(resolve_log_dir)
}

fn today_log_filename() -> String {
    format!("openresquesim-{}.log", Local::now().format("%Y-%m-%d"))
}

fn open_log_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    OpenOptions::new().create(true).append(true).open(path)
}

struct DailyWriterState {
    dir: PathBuf,
    day: String,
    file: std::fs::File,
}

#[derive(Clone)]
struct DailyFileWriter {
    inner: Arc<Mutex<DailyWriterState>>,
}

impl DailyFileWriter {
    fn new(dir: PathBuf) -> std::io::Result<Self> {
        let day = Local::now().format("%Y-%m-%d").to_string();
        let file = open_log_file(&dir.join(format!("openresquesim-{day}.log")))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(DailyWriterState { dir, day, file })),
        })
    }
}

impl std::io::Write for DailyFileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut st = self.inner.lock().expect("daily log writer lock");
        let today = Local::now().format("%Y-%m-%d").to_string();
        if st.day != today {
            st.file.flush()?;
            st.file = open_log_file(&st.dir.join(format!("openresquesim-{today}.log")))?;
            st.day = today;
        }
        st.file.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut st = self.inner.lock().expect("daily log writer lock");
        st.file.flush()
    }
}

/// Initialisiert tracing und schreibt parallel in eine Logdatei.
/// Die Datei wird beim Start geleert.
pub fn init_tracing_with_file() -> PathBuf {
    let file_from_env = env::var("ORS_LOG_FILE")
        .ok()
        .filter(|s| !s.trim().is_empty());

    let (log_path, make_writer): (PathBuf, BoxMakeWriter) = if let Some(path) = file_from_env {
        let path = PathBuf::from(path);
        let _ = std::fs::create_dir_all(path.parent().unwrap_or_else(|| std::path::Path::new(".")));
        let _ = LOG_FILE_PATH.set(path.clone());
        let file = open_log_file(&path).expect("cannot open fixed log file");
        let file_arc = Arc::new(Mutex::new(file));
        let mw = BoxMakeWriter::new({
            let file_arc = file_arc.clone();
            move || {
                file_arc
                    .lock()
                    .expect("logfile lock")
                    .try_clone()
                    .expect("clone logfile")
            }
        });
        (path, mw)
    } else {
        let dir = resolve_log_dir();
        let _ = LOG_DIR_PATH.set(dir.clone());
        let log_path = dir.join(today_log_filename());
        let _ = LOG_FILE_PATH.set(log_path.clone());
        let daily = DailyFileWriter::new(dir).expect("cannot init daily log writer");
        let mw = BoxMakeWriter::new(move || daily.clone());
        (log_path, mw)
    };

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,openresquesim=debug"));

    let _ = fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_writer(make_writer.and(std::io::stdout))
        .with_ansi(false)
        .try_init();

    log_path
}

fn tail_lines(buf: &[u8], max_lines: usize) -> Vec<String> {
    if buf.is_empty() || max_lines == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    for line in buf.split(|b| *b == b'\n').rev() {
        if line.is_empty() {
            continue;
        }
        lines.push(String::from_utf8_lossy(line).to_string());
        if lines.len() >= max_lines {
            break;
        }
    }
    lines.reverse();
    lines
}

async fn read_new_lines(path: &PathBuf, offset: &mut u64) -> anyhow::Result<Vec<String>> {
    use std::io::SeekFrom;
    use tokio::fs::File;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncSeekExt;

    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(_) => return Ok(Vec::new()),
    };
    let len = meta.len();
    if len < *offset {
        *offset = 0;
    }
    if len == *offset {
        return Ok(Vec::new());
    }

    let mut f = File::open(path).await?;
    f.seek(SeekFrom::Start(*offset)).await?;
    let mut buf = vec![0u8; (len - *offset) as usize];
    f.read_exact(&mut buf).await?;
    *offset = len;
    Ok(String::from_utf8_lossy(&buf)
        .split('\n')
        .filter(|l| !l.is_empty())
        .map(|s| s.to_string())
        .collect())
}

/// Streamt die Logdatei per WebSocket (live tail).
pub async fn stream_logs_ws(mut socket: WebSocket, log_path: PathBuf) {
    // Ok(false) heißt: Datei fehlt (z.B. keine Schreibrechte im Logordner)
    let missing = match tokio::fs::try_exists(&log_path).await {
        Ok(true) => None,
        Ok(false) => Some("Datei existiert nicht".to_string()),
        Err(e) => Some(e.to_string()),
    };
    if let Some(e) = missing {
        let _ = socket
            .send(Message::Text(format!(
                "Logdatei nicht verfügbar: {log_path:?} ({e})"
            )))
            .await;
        let _ = socket.close().await;
        return;
    }

    let mut offset = 0u64;
    if let Ok(buf) = tokio::fs::read(&log_path).await {
        for line in tail_lines(&buf, 400) {
            let _ = socket.send(Message::Text(line)).await;
        }
        offset = buf.len() as u64;
    }

    let mut ticker: Interval = tokio::time::interval(Duration::from_millis(800));

    loop {
        select! {
            _ = ticker.tick() => {
                match read_new_lines(&log_path, &mut offset).await {
                    Ok(lines) => {
                        for line in lines {
                            if socket.send(Message::Text(line)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = socket.send(Message::Text(format!("Log-Lese-Fehler: {e}"))).await;
                        break;
                    }
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    let _ = socket.close().await;
}
