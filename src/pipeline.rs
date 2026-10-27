// src/pipeline.rs
use anyhow::{anyhow, Result};
use glib::prelude::*; // ObjectExt::set_property, list_properties
use glib::ObjectExt;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer::prelude::ElementExt;
use gstreamer::prelude::ElementExtManual;
use serde_json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use url::Url;

#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

/// Maximale Anzahl Kamera-Slots im Compositor.
pub const MAX_SOURCES: usize = 6;

/// Namen von GStreamer-Elementen, die gerade im laufenden Betrieb abgebaut werden
/// (Live-Quellentausch). Der Bus-Watch in logging.rs ignoriert Fehler dieser
/// Elemente, damit kein Auto-Reset der Pipeline ausgelöst wird.
static TEARDOWN_NAMES: once_cell::sync::Lazy<Mutex<std::collections::HashSet<String>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(std::collections::HashSet::new()));

/// Zustand des optionalen Verteiler-Ausgangs (RTMP an einen externen Streaming-Server)
/// für Systemseite/UI. `state`: "disabled" | "connecting" | "streaming" | "error".
#[derive(Clone, Default, serde::Serialize)]
pub struct DistributorStatus {
    pub enabled: bool,
    /// Ziel ohne Stream-Key (letzter Pfadteil wird maskiert)
    pub url: String,
    pub state: String,
    pub last_error: Option<String>,
    pub updated_at: i64,
}

static DISTRIBUTOR_STATUS: once_cell::sync::Lazy<Mutex<DistributorStatus>> =
    once_cell::sync::Lazy::new(|| {
        Mutex::new(DistributorStatus {
            state: "disabled".into(),
            ..Default::default()
        })
    });

pub fn distributor_status() -> DistributorStatus {
    DISTRIBUTOR_STATUS
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default()
}

fn set_distributor_status(f: impl FnOnce(&mut DistributorStatus)) {
    if let Ok(mut s) = DISTRIBUTOR_STATUS.lock() {
        f(&mut s);
        s.updated_at = chrono::Utc::now().timestamp();
    }
}

fn set_distributor_disabled() {
    set_distributor_status(|s| {
        s.enabled = false;
        s.url.clear();
        s.state = "disabled".into();
        s.last_error = None;
    });
}

/// RTMP-URLs enthalten oft den Stream-Key als letzten Pfadteil (z.B. bei YouTube).
/// Der Status ist für alle eingeloggten Nutzer sichtbar, daher wird er maskiert.
pub(crate) fn mask_stream_key(url: &str) -> String {
    match url.rsplit_once('/') {
        Some((head, key)) if !key.is_empty() && head.contains("://") && head.matches('/').count() >= 3 => {
            format!("{head}/••••")
        }
        _ => url.to_string(),
    }
}

pub fn is_being_torn_down(name: &str) -> bool {
    TEARDOWN_NAMES
        .lock()
        .map(|s| s.contains(name))
        .unwrap_or(false)
}

fn mark_teardown(names: &[String], on: bool) {
    if let Ok(mut s) = TEARDOWN_NAMES.lock() {
        for n in names {
            if on {
                s.insert(n.clone());
            } else {
                s.remove(n);
            }
        }
    }
}

/// =================================================
/// Öffentliche Types
/// =================================================

#[derive(Clone, Debug, Default)]
pub struct SlotGeom {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub z: i32,
}

pub type BranchCleanup = Box<dyn FnOnce() + Send + 'static>;
pub type SharedCleanup = Arc<Mutex<Option<BranchCleanup>>>;

#[derive(Clone, Debug, Default)]
pub struct SourceSpec {
    pub name: Option<String>,
    pub uri: Option<String>,
    pub dev: Option<String>,
    pub is_live: bool,
    pub pattern: Option<i32>,
    pub is_placeholder: bool,
    pub placeholder_label: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct RecordOptions {
    pub enabled: bool,
    pub directory: String,
    pub basename: String,
    pub container: String,
    pub split_ms: i64,
    pub max_files: i32,
    pub audio_dev: Option<String>,
}

/// =================================================
/// PipelineManager
/// =================================================

pub struct PipelineManager {
    pub pipeline: Option<gst::Pipeline>,
    pub last_layout: Option<Vec<SlotGeom>>,
    pub bus_guard: Option<crate::logging::GstBusGuard>,
    slot_sources: HashMap<i32, Option<String>>,
    webrtcbin: Option<gst::Element>,
    audio_src: Option<gst::Element>,
    audio_tee: Option<gst::Element>, // RTP/Opus
    // Roh-Audio-Tee (vor Opus), z.B. für VLC/MPEG-TS
    audio_tee_raw: Option<gst::Element>,
    audio_gain: f64, // linear gain (1.0 = 100%)
    audio_muted: bool,
    layout_slots: usize, // Anzahl der aktuell erwarteten Layout-Slots
    metrics: Arc<Mutex<PipelineMetrics>>,
    webrtc_cleanups: Vec<SharedCleanup>,
    // Generation des Kamerazweigs pro Slot (für Live-Quellentausch; 0 = Erstaufbau)
    slot_gen: HashMap<i32, u32>,
}

#[derive(Clone, Debug)]
pub struct PipelineMetrics {
    pub fps: f64,
    pub dropped: u64,
    frame_count: u64,
    last_ts: Instant,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AudioInputSpec {
    pub device: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub gain: Option<f64>, // linear factor (1.0 = 100%)
    /// Hardware-Kanäle (1-basiert), die in den Stereo-Stream gehen. Leer/None = alle Kanäle
    /// gemischt. Genau zwei Kanäle = Stereo (erster links, zweiter rechts), sonst Mono auf beide Seiten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<u32>>,
}

impl Default for PipelineMetrics {
    fn default() -> Self {
        Self {
            fps: 0.0,
            dropped: 0,
            frame_count: 0,
            last_ts: Instant::now(),
        }
    }
}

impl PipelineManager {
    pub fn new() -> Self {
        let _ = gst::init();
        Self {
            pipeline: None,
            last_layout: None,
            bus_guard: None,
            slot_sources: HashMap::new(),
            webrtcbin: None,
            audio_src: None,
            audio_tee: None,
            audio_tee_raw: None,
            audio_gain: 1.0,
            audio_muted: false,
            layout_slots: 1, // Default: mindestens 1 Slot
            metrics: Arc::new(Mutex::new(PipelineMetrics::default())),
            webrtc_cleanups: Vec::new(),
            slot_gen: HashMap::new(),
        }
    }

    pub fn current_layout_slots(&self) -> usize {
        if self.layout_slots > 0 {
            self.layout_slots
        } else if let Some(v) = &self.last_layout {
            v.len().max(1)
        } else {
            1
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rebuild<T>(
        &mut self,
        _status: &T,
        out_rtmp: Option<&str>,
        audio_dev: Option<&str>,
        rec_opt_external: Option<RecordOptions>,
    ) -> Result<()> {
        self.stop()?;

        if let Some(r) = rec_opt_external.as_ref().filter(|r| r.enabled) {
            info!(
                dir=%r.directory,
                base=%r.basename,
                fmt=%r.container,
                seg_ms=r.split_ms,
                max_files=r.max_files,
                "⏺️ Aufnahme angefordert"
            );
        }

        // Slots → Quellen
        let mut cameras: Vec<SourceSpec> = Vec::new();
        if self.slot_sources.is_empty() {
            cameras.push(SourceSpec {
                name: Some("testsrc".into()),
                is_live: true,
                ..Default::default()
            });
        } else {
            let mut keyed: Vec<(i32, Option<String>)> = self
                .slot_sources
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect();
            keyed.sort_by_key(|(k, _)| *k);

            for (place, src) in keyed {
                cameras.push(SourceSpec {
                    // WICHTIG: Name NICHT = URL setzen, sonst landet die URL im Overlay.
                    name: None,
                    uri: src.clone(),
                    is_live: true,
                    // Neutraler, nicht-geheimer Label für Overlay / Fallback
                    placeholder_label: Some(format!("Kamera Slot {}", place)),
                    ..Default::default()
                });
            }
        }

        // 🔥 NEU: Layout entscheidet, wie viele Inputs gebaut werden
        let layout_slots = self.current_layout_slots();

        let p = build_pipeline(
            cameras,
            rec_opt_external,
            out_rtmp,
            layout_slots,
            audio_dev,
            self.metrics.clone(),
        )?;

        if let Some(guard) = crate::logging::attach_gst_bus(&p) {
            info!("✅ GST bus watch attached");
            self.bus_guard = Some(guard);
        }

        p.set_state(gst::State::Playing)?;
        let state = p.current_state();
        info!(?state, "pipeline state after set_state");
        self.pipeline = Some(p);
        self.slot_gen.clear(); // alle Slots wieder Generation 0
        // Regler-Stand (Lautstärke/Stumm) auch auf die neu gebaute Aufnahme anwenden
        self.apply_audio_levels();

        if let Some(geoms) = self.last_layout.clone() {
            let _ = self.apply_layout(&geoms);
        }

        Ok(())
    }

    pub fn metrics_snapshot(&self) -> (f64, u64) {
        let m = self.metrics.lock().unwrap();
        (m.fps, m.dropped)
    }

    pub fn apply_layout(&mut self, geoms: &Vec<SlotGeom>) -> Result<()> {
        let Some(p) = &self.pipeline else {
            return Ok(());
        };
        let comp = p
            .by_name("comp")
            .ok_or_else(|| anyhow!("compositor not found"))?;

        let mut sinks: Vec<gst::Pad> = comp
            .pads()
            .into_iter()
            .filter(|pad| pad.direction() == gst::PadDirection::Sink)
            .collect();
        sinks.sort_by_key(|pad| sink_index(&pad.name()));

        let mut applied = 0usize;
        // Slots, für die das Layout keine Geometrie hat (z.B. nach Raumwechsel auf
        // ein kleineres Layout), unsichtbar machen statt sie stehen zu lassen.
        for pad in sinks.iter().skip(geoms.len()) {
            pad.set_property("alpha", 0.0f64);
            pad.set_property("zorder", 0u32);
            debug!(pad = %pad.name(), "📐 layout slot hidden (no geometry)");
        }
        for (i, g) in geoms.iter().enumerate() {
            if let Some(pad) = sinks.get(i) {
                pad.set_property("xpos", g.x);
                pad.set_property("ypos", g.y);
                pad.set_property("width", g.w);
                pad.set_property("height", g.h);
                pad.set_property("zorder", g.z.max(0) as u32);
                pad.set_property("alpha", 1.0f64);
                applied += 1;
                debug!(
                    slot = i,
                    pad = %pad.name(),
                    x = g.x,
                    y = g.y,
                    w = g.w,
                    h = g.h,
                    z = g.z,
                    "📐 layout slot"
                );
            }
        }
        info!(pads = applied, "📐 layout applied");
        self.last_layout = Some(geoms.clone());
        self.layout_slots = geoms.len().max(1); // 🔥 NEU: Zahl der Slots merken
        Ok(())
    }

    pub fn set_cached_layout(&mut self, geoms: Vec<SlotGeom>) {
        // Layout auch ohne aktive Pipeline merken, damit beim nächsten build die Slot-Anzahl stimmt.
        self.layout_slots = geoms.len().max(1);
        self.last_layout = Some(geoms.clone());
        let _ = self.apply_layout(&geoms);
    }

    pub fn set_source(&mut self, place: i32, source: Option<String>) -> Result<()> {
        self.slot_sources.insert(place, source);
        Ok(())
    }

    /// Anzahl der Slots (Compositor-Eingänge) in der laufenden Pipeline, 0 ohne Pipeline.
    pub fn live_slot_count(&self) -> usize {
        self.pipeline
            .as_ref()
            .and_then(|p| p.by_name("comp"))
            .map(|c| c.sink_pads().len())
            .unwrap_or(0)
    }

    /// Stellt sicher, dass die laufende Pipeline mindestens `n` Slots hat.
    /// Fehlende Slots werden im laufenden Betrieb ergänzt (Quelle aus `slot_sources`)
    /// und zunächst unsichtbar (alpha 0) an den Compositor gehängt; `apply_layout`
    /// blendet sie anschließend ein.
    fn ensure_slots_live(&mut self, n: usize) -> Result<()> {
        let Some(p) = self.pipeline.clone() else {
            return Ok(());
        };
        let comp = p
            .by_name("comp")
            .ok_or_else(|| anyhow!("compositor not found"))?;
        let existing = comp.sink_pads().len();
        let target = n.max(1).min(MAX_SOURCES);
        for i in existing..target {
            let idx = i as i32;
            let uri = self.slot_sources.get(&idx).cloned().flatten();
            let label = format!("Kamera Slot {idx}");
            let spec = SourceSpec {
                uri,
                is_live: true,
                placeholder_label: Some(label.clone()),
                ..Default::default()
            };
            let bin = build_source_bin(&spec, idx, label)?;
            p.add(&bin)?;
            let sink = comp
                .request_pad_simple("sink_%u")
                .ok_or_else(|| anyhow!("compositor sink pad"))?;
            sink.set_property("alpha", 0.0f64);
            sink.set_property("zorder", 0u32);
            let srcpad = bin
                .static_pad("src")
                .ok_or_else(|| anyhow!("srcbin src pad"))?;
            srcpad.link(&sink)?;
            bin.sync_state_with_parent()?;
            self.slot_gen.insert(idx, 0);
            info!(slot = idx, pad = %sink.name(), "🎥 slot added live");
        }
        Ok(())
    }

    /// Tauscht die Quelle eines Slots im laufenden Betrieb, ohne die Pipeline neu zu
    /// bauen. Ablauf: Fallback (Schwarzbild) aktiv schalten → alten Kamerazweig
    /// blockieren, vom Selector trennen und im Hintergrund auf NULL setzen → neuen
    /// Zweig einhängen. Der Selector schaltet beim ersten Frame automatisch auf die
    /// neue Kamera. Encoder, Aufnahme und WebRTC bleiben unberührt.
    pub fn swap_slot_source_live(&mut self, place: i32, source: Option<String>) -> Result<()> {
        self.slot_sources.insert(place, source.clone());
        let Some(p) = self.pipeline.clone() else {
            return Ok(());
        };
        let bin = p
            .by_name(&format!("srcbin_{place}"))
            .and_then(|e| e.downcast::<gst::Bin>().ok())
            .ok_or_else(|| anyhow!("Slot-Bin nicht gefunden (kein Live-Tausch möglich)"))?;
        let sel = bin
            .by_name(&format!("sel_{place}"))
            .ok_or_else(|| anyhow!("Selector nicht gefunden"))?;
        let fb_pad = bin
            .by_name(&format!("fb_cf_{place}"))
            .and_then(|cf| cf.static_pad("src"))
            .and_then(|sp| sp.peer())
            .ok_or_else(|| anyhow!("Fallback-Pad nicht gefunden"))?;

        // 1) Fallback zeigen, solange getauscht wird
        sel.set_property("active-pad", &fb_pad);

        // 2) alten Kamerazweig abbauen
        let old_gen = self.slot_gen.get(&place).copied().unwrap_or(0);
        teardown_cam_branch(&bin, &sel, &fb_pad, old_gen);

        // 3) neuen Kamerazweig aufbauen
        let new_gen = old_gen + 1;
        self.slot_gen.insert(place, new_gen);
        let uri = source.unwrap_or_default();
        let Some(elems) = build_cam_branch(&bin, &uri, place, new_gen)? else {
            info!(slot = place, "🎥 slot switched to fallback (no source)");
            return Ok(());
        };
        let cam_last = elems
            .last()
            .ok_or_else(|| anyhow!("cam branch empty"))?;
        attach_cam_to_selector(&sel, &fb_pad, cam_last)?;
        // Zustand synchronisieren: Senke zuerst, Quelle zuletzt
        for e in elems.iter().rev() {
            e.sync_state_with_parent()?;
        }
        info!(slot = place, gen = new_gen, uri = %redact_uri(&uri), "🎥 slot source swapped live");
        Ok(())
    }

    /// Raumwechsel im laufenden Betrieb: fehlende Slots ergänzen, Quellen je Slot
    /// tauschen (nur wenn sie sich ändern) und Layout anwenden. Encoder, Aufnahme und
    /// WebRTC laufen ununterbrochen weiter. Gibt Warnungen je fehlgeschlagenem Slot zurück.
    pub fn switch_room_live(
        &mut self,
        sources: &[Option<String>],
        geoms: Vec<SlotGeom>,
    ) -> Result<Vec<String>> {
        if self.pipeline.is_none() {
            return Err(anyhow!("keine aktive Pipeline"));
        }
        let slots = geoms.len().max(1).min(MAX_SOURCES);
        let desired = |i: usize| -> Option<String> {
            if i < slots {
                sources
                    .get(i)
                    .cloned()
                    .flatten()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            } else {
                None
            }
        };

        // Neue Slots vorbelegen, damit ensure_slots_live sie direkt mit der richtigen Quelle baut
        let existing = self.live_slot_count();
        for i in existing..slots {
            self.slot_sources.insert(i as i32, desired(i));
        }
        self.ensure_slots_live(slots)?;

        let mut warnings = Vec::new();
        let live = self.live_slot_count();
        for i in 0..existing.min(live) {
            let want = desired(i);
            let have = self
                .slot_sources
                .get(&(i as i32))
                .cloned()
                .flatten()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            if want == have {
                debug!(slot = i, "room switch: slot unchanged");
                continue;
            }
            if let Err(e) = self.swap_slot_source_live(i as i32, want) {
                warn!(slot = i, ?e, "room switch: slot swap failed");
                warnings.push(format!("Slot {}: {}", i + 1, e));
            }
        }
        // Slots jenseits der Live-Pads für einen späteren Rebuild konsistent halten
        for i in live..64 {
            self.slot_sources.insert(i as i32, desired(i));
        }
        self.set_cached_layout(geoms);
        info!(slots, live, warnings = warnings.len(), "🏠 room switched live");
        Ok(warnings)
    }

    pub fn stop(&mut self) -> Result<()> {
        // Erst die ganze Pipeline synchron auf NULL, dann die Zuschauer-Zweige abbauen.
        // Umgekehrt liefe deren (asynchrones) Aufräumen auf dem GLib-Thread gleichzeitig
        // mit dem NULL-Wechsel hier – zwei Threads änderten denselben webrtcbin, der dann
        // mitten im Zustandswechsel freigegeben wurde (Absturz „g_mutex_clear() called on
        // uninitialised or locked mutex“).
        let p_opt = self.pipeline.take();
        if let Some(p) = &p_opt {
            let _ = p.set_state(gst::State::Null);
        }
        self.run_cleanups();
        if let Some(p) = p_opt {
            // Der Verteiler-Zweig ist vom Pipeline-Zustand entkoppelt (siehe
            // build_rtmp_output) und muss eigens heruntergefahren werden.
            for name in [
                "rtmp_sink", "rtmp_mux", "rtmp_aacparse", "rtmp_aac", "rtmp_acaps",
                "rtmp_ares", "rtmp_aconv", "rtmp_aq", "rtmp_parse", "q_rtmp",
            ] {
                if let Some(el) = p.by_name(name) {
                    let _ = el.set_state(gst::State::Null);
                    el.set_locked_state(false);
                }
            }
        }
        set_distributor_disabled();
        self.slot_gen.clear();
        crate::logging::detach_gst_bus(&mut self.bus_guard);
        self.webrtcbin = None;
        self.audio_src = None;
        self.audio_tee = None;
        self.audio_tee_raw = None;
        Ok(())
    }

    pub fn hard_reset(&mut self) -> Result<()> {
        let audio_gain = self.audio_gain;
        let audio_muted = self.audio_muted;
        self.stop()?;
        *self = PipelineManager::new();
        self.audio_gain = audio_gain;
        self.audio_muted = audio_muted;
        info!("🧹 pipeline hard reset done");
        Ok(())
    }

    pub fn register_cleanup(&mut self, cleanup: SharedCleanup) {
        self.webrtc_cleanups.push(cleanup);
    }

    fn run_cleanups(&mut self) {
        let cleanups = std::mem::take(&mut self.webrtc_cleanups);
        for cleanup in cleanups {
            let to_run = cleanup.lock().unwrap().take();
            if let Some(run) = to_run {
                run();
            }
        }
    }

    pub fn latency_snapshot(&self) -> Result<(u64, u64)> {
        Ok((0, 0))
    }

    pub fn set_audio_device(&mut self, device: &str) -> Result<()> {
        if let Some(asrc) = &self.audio_src {
            asrc.set_property_from_str("device", device);
            info!(device, "🔊 audio device set");
        } else {
            warn!("audio device switch requested, but no audio source is active yet");
        }
        Ok(())
    }

    pub fn set_audio_volume(&mut self, volume_percent: f64) -> Result<f64> {
        // clamp 0..400% (0.0..4.0 linear)
        self.audio_gain = (volume_percent / 100.0).clamp(0.0, 4.0);
        self.apply_audio_levels();
        Ok(self.audio_gain * 100.0)
    }

    pub fn toggle_audio_mute(&mut self) -> Result<bool> {
        self.audio_muted = !self.audio_muted;
        self.apply_audio_levels();
        Ok(self.audio_muted)
    }

    /// Aktuelle Stream-Lautstärke (linear) und Stummschaltung – neue Zuschauer-Zweige
    /// übernehmen diese Werte (siehe ws.rs).
    pub fn viewer_audio_level(&self) -> (f64, bool) {
        (self.audio_gain, self.audio_muted)
    }

    /// Lautstärke/Stumm auf alle Zuschauer-Zweige (`webrtc_volume_<id>`) und die Aufnahme
    /// (`rec_volume`) anwenden. Früher wurde nur ein Element `webrtc_volume` gesucht, das
    /// es seit den Zuschauer-Zweigen je Verbindung nicht mehr gibt – der Regler wirkte
    /// dadurch nur auf die Aufnahme.
    fn apply_audio_levels(&self) {
        let Some(p) = &self.pipeline else { return };
        for el in p.children() {
            let name = el.name();
            if name == "rec_volume" || name.starts_with("webrtc_volume_") {
                el.set_property("volume", self.audio_gain);
                el.set_property("mute", self.audio_muted);
            }
        }
    }

    pub fn audio_state(&self) -> (f64, bool) {
        (
            (self.audio_gain * 100.0).clamp(0.0, 400.0),
            self.audio_muted,
        )
    }

}

/// =================================================
/// Helper
/// =================================================

fn sink_index(name: &str) -> i32 {
    if let Some(rest) = name.strip_prefix("sink_") {
        if let Ok(n) = rest.parse::<i32>() {
            return n;
        }
    }
    0
}

fn caps_to_compositor() -> gst::Caps {
    gst::Caps::builder("video/x-raw")
        .field("format", &"ARGB") // wie im Mixer-Output
        // .field("width",   &1920i32)
        // .field("height",  &1080i32)
        // .field("framerate", &gst::Fraction::new(30, 1))
        .build()
}

fn make(factory: &str, name: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .name(name)
        .build()
        .map_err(|_| anyhow!("missing element: {factory}"))
}

fn make_queue(name: &str) -> Result<gst::Element> {
    let q = make("queue", name)?;
    q.set_property_from_str("leaky", "downstream");
    q.set_property("max-size-buffers", 0u32);
    q.set_property("max-size-bytes", 0u32);
    q.set_property("max-size-time", 0u64);
    Ok(q)
}


/// =================================================
/// 1) Inputs
/// =================================================

/// Baut den eigentlichen Kamera-/Streamzweig eines Slots (ohne Fallback) in `bin`
/// und verlinkt ihn intern. Gibt die Elemente in Flussrichtung zurück (Quelle zuerst,
/// Capsfilter zuletzt) oder `None`, wenn `uri` keine echte Kamera beschreibt.
/// `gen` ist die Generation des Zweigs (0 = Erstaufbau, danach je Live-Tausch +1) und
/// landet in den Elementnamen, damit alte und neue Zweige unterscheidbar bleiben.
fn build_cam_branch(
    bin: &gst::Bin,
    uri: &str,
    index: i32,
    gen: u32,
) -> Result<Option<Vec<gst::Element>>> {
    let to_comp_caps = caps_to_compositor();
    let uri = uri.to_string();
    // Nur für echte Kameras/Streams einen Cam-Zweig bauen
    let is_real_cam =
        uri.starts_with("v4l2://") || uri.starts_with("rtsp://") || uri.starts_with("rtsps://");

    let elems: Option<Vec<gst::Element>>;
    if is_real_cam {
        if uri.starts_with("v4l2://") {
            let dev = uri.trim_start_matches("v4l2://");
            let src = make("v4l2src", &format!("v4l2_{index}_g{gen}"))?;
            src.set_property("device", dev);
            src.set_property("is-live", true);
            let conv = make("videoconvert", &format!("v4l2_conv_{index}_g{gen}"))?;
            let scale = make("videoscale", &format!("v4l2_scale_{index}_g{gen}"))?;
            // 🔥 Wichtig: Seitenverhältnis erhalten + schwarze Ränder
            scale.set_property("add-borders", true);

            // let rate = make("videorate", &format!("v4l2_rate_{index}_g{gen}"))?;
            let cf = make("capsfilter", &format!("v4l2_cf_{index}_g{gen}"))?;
            cf.set_property("caps", &to_comp_caps);

            bin.add_many([&src, &conv, &scale, &cf])?;
            gst::Element::link_many([&src, &conv, &scale, &cf])?;

            elems = Some(vec![src, conv, scale, cf]);
        } else {
            // RTSP
            let src = make("rtspsrc", &format!("rtsp_{index}_g{gen}"))?;
            // Credentials und saubere Location extrahieren (verhindert Parse-Fehler)
            // Location direkt setzen (mit ggf. enthaltenen Credentials) – Matches gst-launch Test
            let mut rtsp_location = uri.clone();
            let mut rtsp_user: Option<String> = None;
            let mut rtsp_pass: Option<String> = None;
            if let Ok(u) = Url::parse(&uri) {
                if !u.username().is_empty() {
                    rtsp_user = Some(u.username().to_string());
                }
                if let Some(p) = u.password() {
                    rtsp_pass = Some(p.to_string());
                }
                // Optional: Pfad mit doppeltem Slash, falls die Kamera das erwartet
                if std::env::var("ORS_RTSP_DOUBLE_SLASH")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false)
                {
                    // Insert an extra slash after host if not present
                    if let Some(pos) = rtsp_location.find("://") {
                        // find first slash after host
                        if let Some(p) = rtsp_location[pos + 3..].find('/') {
                            let host_part = &rtsp_location[..pos + 3 + p];
                            let path_part = &rtsp_location[pos + 3 + p..];
                            if !path_part.starts_with("//") {
                                rtsp_location =
                                    format!("{host_part}/{}", path_part.trim_start_matches('/'));
                            }
                        }
                    }
                }
            }
            src.set_property("location", &rtsp_location);
            if let Some(u) = rtsp_user {
                if src.has_property("user-id", None) {
                    src.set_property("user-id", &u);
                }
            }
            if let Some(p) = rtsp_pass {
                if src.has_property("user-pw", None) {
                    src.set_property("user-pw", &p);
                }
            }
            if src.has_property("allow-basic-auth", None) {
                src.set_property("allow-basic-auth", true);
            }

            // Stabiler machen: Protokoll konfigurierbar
            // ORS_RTSP_PROTOCOLS=tcp|udp|udp+tcp (default tcp)
            // Default: udp (per Env überschreibbar)
            let proto = std::env::var("ORS_RTSP_PROTOCOLS").unwrap_or_else(|_| "udp".to_string());
            if src.has_property("protocols", None) {
                src.set_property_from_str("protocols", &proto);
            }
            // Latenz moderat (gst-launch erfolgreich mit 500ms)
            src.set_property("latency", 500u32);
            src.set_property("timeout", 30_000_000_000u64); // 30s
            src.set_property("tcp-timeout", 30_000_000_000u64); // 30s
            if src.has_property("udp-reconnect", None) {
                src.set_property("udp-reconnect", true);
            }
            if src.has_property("do-retransmission", None) {
                src.set_property("do-retransmission", true);
            }
            if src.has_property("do-rtsp-keep-alive", None) {
                src.set_property("do-rtsp-keep-alive", true);
            }
            if std::env::var("ORS_RTSP_SHORT_HEADER")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
            {
                if src.has_property("short-header", None) {
                    src.set_property("short-header", true);
                }
            }
            // Optionale TLS-Entschärfung für selbst-signierte RTSP-Server:
            // setze ORS_RTSP_INSECURE=1 um Zertifikatsprüfung abzuschalten
            if std::env::var("ORS_RTSP_INSECURE")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
            {
                if src.has_property("tls-validation-flags", None) {
                    src.set_property("tls-validation-flags", 0u32);
                }
            }
            // Optional: nur setzen, wenn Property existiert
            if src.has_property("retry", None) {
                src.set_property("retry", 10u32);
            } else {
                tracing::warn!(
                    "rtspsrc: property 'retry' nicht vorhanden – ältere GStreamer-Version?"
                );
            }

            // Jitterbuffer vor depay
            let jb = make("rtpjitterbuffer", &format!("rtsp_jb_{index}_g{gen}"))?;
            jb.set_property("latency", 250u32);
            // Mode 4 = synced; setze per Enum-String falls vorhanden
            if jb.has_property("mode", None) {
                jb.set_property_from_str("mode", "synced");
            }

            let depay = make("rtph264depay", &format!("rtsp_depay_{index}_g{gen}"))?;
            let parse = make("h264parse", &format!("rtsp_parse_{index}_g{gen}"))?;
            let dec = make("avdec_h264", &format!("rtsp_dec_{index}_g{gen}"))?;
            let conv = make("videoconvert", &format!("rtsp_conv_{index}_g{gen}"))?;
            let scale = make("videoscale", &format!("rtsp_scale_{index}_g{gen}"))?;
            // 🔥 Auch hier: Letterboxing statt Stretch
            scale.set_property("add-borders", true);

            // let rate = make("videorate", &format!("rtsp_rate_{index}_g{gen}"))?;
            let cf = make("capsfilter", &format!("rtsp_cf_{index}_g{gen}"))?;
            cf.set_property("caps", &to_comp_caps);

            bin.add_many([&src, &jb, &depay, &parse, &dec, &conv, &scale, &cf])?;
            gst::Element::link_many([&jb, &depay, &parse, &dec, &conv, &scale, &cf])?;

            let depay_sink = depay
                .static_pad("sink")
                .ok_or_else(|| anyhow!("rtsp depay sink"))?;
            let depay_sink_weak = depay_sink.downgrade();
            let jb_for_link = jb.clone();
            src.connect_pad_added(move |_s, pad| {
                if pad.name().starts_with("recv_rtp_src") {
                    if let Some(sink) = depay_sink_weak.upgrade() {
                        if let Some(jb_sink) = jb_for_link.static_pad("sink") {
                            let _ = pad.link(&jb_sink);
                        } else {
                            let _ = pad.link(&sink);
                        }
                    }
                }
            });

            elems = Some(vec![src, jb, depay, parse, dec, conv, scale, cf]);
        }
    } else {
        // KEINE echte Quelle → nur Fallback verwenden
        elems = None;
    }


    Ok(elems)
}

/// Hängt das letzte Zweig-Element (`cam_last`) an einen neuen Selector-Eingang und
/// schaltet auf diesen um, sobald der erste Buffer ankommt. Bis dahin bleibt der
/// Fallback (`sel_sink_fb`) aktiv. Gibt den neuen Selector-Sink-Pad zurück.
fn attach_cam_to_selector(
    sel: &gst::Element,
    sel_sink_fb: &gst::Pad,
    cam_last: &gst::Element,
) -> Result<gst::Pad> {
    let sel_sink_cam = sel
        .request_pad_simple("sink_%u")
        .ok_or_else(|| anyhow!("selector sink pad (cam)"))?;
    let cam_srcpad = cam_last
        .static_pad("src")
        .ok_or_else(|| anyhow!("cam last src pad"))?;
    cam_srcpad.link(&sel_sink_cam)?;

    // Default: Fallback aktiv (bis Cam liefert)
    sel.set_property("active-pad", sel_sink_fb);

    // Sobald Cam das erste Buffer liefert -> auf Cam umschalten
    let sel_weak = sel.downgrade();
    let sel_sink_cam_weak = sel_sink_cam.downgrade();
    cam_srcpad.add_probe(gst::PadProbeType::BUFFER, move |_pad, _info| {
        if let (Some(sel), Some(cam_sink)) = (sel_weak.upgrade(), sel_sink_cam_weak.upgrade()) {
            let _ = sel.set_property("active-pad", &cam_sink);
        }
        gst::PadProbeReturn::Remove
    });
    Ok(sel_sink_cam)
}

/// Baut den Kamerazweig der Generation `gen` in einem laufenden Slot-Bin ab.
/// Der Zweig wird per IDLE-Probe blockiert und vom Selector getrennt; danach setzt
/// ein Hintergrund-Thread die Elemente auf NULL und entfernt sie aus dem Bin
/// (rtspsrc-Teardown kann blockieren und darf den Aufrufer nicht aufhalten).
/// Solange der Abbau läuft, ignoriert der Bus-Watch Fehler dieser Elemente.
fn teardown_cam_branch(bin: &gst::Bin, sel: &gst::Element, fb_pad: &gst::Pad, gen: u32) {
    let suffix = format!("_g{gen}");
    let mut elems: Vec<gst::Element> = bin
        .iterate_elements()
        .into_iter()
        .filter_map(|r| r.ok())
        .filter(|e| e.name().ends_with(&suffix))
        .collect();
    if elems.is_empty() {
        return;
    }
    let names: Vec<String> = elems.iter().map(|e| e.name().to_string()).collect();
    mark_teardown(&names, true);

    // Selector-Eingänge des alten Zweigs (alles außer Fallback) blockieren + trennen
    let mut branch_tail: Option<gst::Element> = None;
    for sp in sel.sink_pads() {
        if &sp == fb_pad {
            continue;
        }
        let Some(peer) = sp.peer() else {
            sel.release_request_pad(&sp);
            continue;
        };
        let parent = peer.parent_element();
        let belongs = parent
            .as_ref()
            .map(|e| e.name().ends_with(&suffix))
            .unwrap_or(false);
        if !belongs {
            continue;
        }
        if branch_tail.is_none() {
            branch_tail = parent;
        }
        let sel_c = sel.clone();
        let sp_c = sp.clone();
        // IDLE-Probe: läuft, sobald der Zweig nicht mitten in einem Push steckt.
        // Rückgabe Ok hält den Pad blockiert, bis set_state(NULL) ihn deaktiviert.
        peer.add_probe(gst::PadProbeType::IDLE, move |pad, _info| {
            let _ = pad.unlink(&sp_c);
            sel_c.release_request_pad(&sp_c);
            gst::PadProbeReturn::Ok
        });
    }

    // Abbau-Reihenfolge: vom Selector-Ende rückwärts (Senke zuerst, Quelle zuletzt).
    // Der blockierte Streaming-Thread steckt im letzten Element; wird das zuerst
    // deaktiviert, kommt er frei, und Upstream-Elemente können ihre Tasks sauber
    // stoppen. Umgekehrt würde z.B. der Jitterbuffer ewig auf seinen Task warten.
    let mut ordered: Vec<gst::Element> = Vec::new();
    let mut cur = branch_tail;
    while let Some(e) = cur {
        if ordered.iter().any(|x| x == &e) {
            break;
        }
        ordered.push(e.clone());
        cur = e
            .sink_pads()
            .first()
            .and_then(|sp| sp.peer())
            .and_then(|p| p.parent_element());
    }
    // Rest (z.B. rtspsrc, dessen Pad dynamisch verlinkt wird) hinten anhängen
    elems.retain(|e| !ordered.iter().any(|x| x == e));
    elems.sort_by_key(|e| if e.sink_pads().is_empty() { 1 } else { 0 });
    ordered.extend(elems);
    let elems = ordered;

    let bin_c = bin.clone();
    let spawned = std::thread::Builder::new()
        .name("ors-slot-teardown".into())
        .spawn(move || {
            for e in &elems {
                let _ = e.set_locked_state(true);
                if let Err(err) = e.set_state(gst::State::Null) {
                    warn!(element = %e.name(), ?err, "slot teardown: set_state(NULL) failed");
                }
            }
            for e in &elems {
                let _ = bin_c.remove(e);
            }
            mark_teardown(&names, false);
            debug!(count = elems.len(), "🧹 old cam branch removed");
        });
    if let Err(e) = spawned {
        error!(?e, "slot teardown: thread spawn failed");
    }
}

/// URI ohne Zugangsdaten für Logausgaben.
fn redact_uri(uri: &str) -> String {
    if let Ok(mut u) = Url::parse(uri) {
        if !u.username().is_empty() || u.password().is_some() {
            let _ = u.set_username("");
            let _ = u.set_password(None);
        }
        return u.to_string();
    }
    uri.to_string()
}

/// URI eines SourceSpec auflösen (uri > dev > leer).
fn resolve_spec_uri(spec: &SourceSpec) -> String {
    if let Some(u) = &spec.uri {
        u.clone()
    } else if let Some(dev) = &spec.dev {
        if dev.starts_with("v4l2://") {
            dev.clone()
        } else {
            format!("v4l2://{dev}")
        }
    } else {
        String::new()
    }
}

fn build_source_bin(spec: &SourceSpec, index: i32, label_when_missing: String) -> Result<gst::Bin> {
    use gst::prelude::*;
    let bin = gst::Bin::new();
    bin.set_property("name", &format!("srcbin_{index}"));

    // Einheitliche Ziel-Caps zum Compositor
    let to_comp_caps = caps_to_compositor();

    // Selector
    let sel = gst::ElementFactory::make("input-selector")
        .name(format!("sel_{index}"))
        // ggf. diese Zeile weglassen, falls Property nicht existiert:
        // .property("always-live", true)
        .build()
        .map_err(|_| anyhow!("missing element: input-selector"))?;
    bin.add(&sel)?;

    // ---------------------------
    // Fallback: Schwarz + Text
    // ---------------------------

    let fb_src = gst::ElementFactory::make("videotestsrc")
        .property("is-live", true)
        .property_from_str("pattern", "black")
        .build()?;
    let fb_conv = make("videoconvert", &format!("fb_conv_{index}"))?;
    let fb_text = make("textoverlay", &format!("fb_text_{index}"))?;
    fb_text.set_property("text", &label_when_missing);
    fb_text.set_property_from_str("halignment", "center"); // ok
    fb_text.set_property_from_str("valignment", "top"); // ✅ use top/baseline/bottom
    fb_text.set_property("draw-shadow", true);
    fb_text.set_property("font-desc", "Sans 36");
    let fb_cf = make("capsfilter", &format!("fb_cf_{index}"))?;
    fb_cf.set_property("caps", &to_comp_caps);

    bin.add_many([&fb_src, &fb_conv, &fb_text, &fb_cf])?;
    gst::Element::link_many([&fb_src, &fb_conv, &fb_text, &fb_cf])?;

    // In den Selector
    let sel_sink_fb = sel
        .request_pad_simple("sink_%u")
        .ok_or_else(|| anyhow!("selector sink pad (fb)"))?;
    let fb_srcpad = fb_cf
        .static_pad("src")
        .ok_or_else(|| anyhow!("fb cf src pad"))?;
    fb_srcpad.link(&sel_sink_fb)?;

    // ---------------------------
    // Kamera-/Quell-Zweig (ohne Text)
    // ---------------------------
    let uri = resolve_spec_uri(spec);
    match build_cam_branch(&bin, &uri, index, 0)? {
        Some(elems) => {
            let cam_last = elems.last().ok_or_else(|| anyhow!("cam branch empty"))?;
            attach_cam_to_selector(&sel, &sel_sink_fb, cam_last)?;
        }
        None => {
            // Keine Cam: dauerhaft Fallback
            sel.set_property("active-pad", &sel_sink_fb);
        }
    }

    // Ghost src vom Selector
    let sel_src = sel
        .static_pad("src")
        .ok_or_else(|| anyhow!("selector src"))?;
    let ghost = gst::GhostPad::with_target(&sel_src)?;
    ghost.set_active(true)?;
    bin.add_pad(&ghost)?;

    Ok(bin)
}

// Globale Audioquelle (mixer) nur einmal anlegen und verteilen
fn build_shared_audio_src(
    pipeline: &gst::Pipeline,
    audio_dev: Option<&str>,
) -> Result<gst::Element> {
    // Wenn schon vorhanden, den Tee zurückgeben
    if let Some(existing) = pipeline.by_name("shared_audio_tee") {
        return Ok(existing);
    }

    let disable_audio = std::env::var("OPENRESQUESIM_DISABLE_AUDIO")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let force_testsrc = std::env::var("ORS_AUDIO_TESTSRC")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    // Device-Liste: JSON aus audio_dev oder ENV
    let dev_env = std::env::var("OPENRESQUESIM_ALSA_DEVICE").ok();
    let dev_legacy = std::env::var("ORS_AUDIO_DEVICE").ok();
    let merged = audio_dev.or(dev_env.as_deref()).or(dev_legacy.as_deref());
    let merged_norm = normalize_audio_device_config(merged);
    let specs = parse_audio_inputs(merged_norm.as_deref());
    info!(
        disable_audio,
        force_testsrc,
        audio_dev = audio_dev.unwrap_or("-"),
        env_device = dev_env.as_deref().unwrap_or("-"),
        legacy_device = dev_legacy.as_deref().unwrap_or("-"),
        merged = merged_norm.as_deref().unwrap_or("-"),
        inputs = specs.len(),
        "🔊 audio: config"
    );

    // Mixer + Tee
    let mixer = make("audiomixer", "shared_audiomixer")?;
    // Mixer-Caps (Output)
    let mix_caps = make("capsfilter", "shared_mix_caps")?;
    mix_caps.set_property(
        "caps",
        &gst::Caps::builder("audio/x-raw")
            .field("format", &"S16LE")
            .field("rate", &48_000i32)
            .field("channels", &2i32)
            .build(),
    );
    let mix_conv = make("audioconvert", "shared_mix_conv")?;
    let mix_res = make("audioresample", "shared_mix_res")?;
    let tee = make("tee", "shared_audio_tee")?;
    // Ohne Abnehmer (keine Aufnahme, kein Zuschauer verbunden) darf der Audio-Mixer nicht
    // mit not-linked stehen bleiben – sonst bekommen später verbundene Zuschauer keinen Ton.
    tee.set_property("allow-not-linked", true);

    pipeline.add_many([&mixer, &mix_caps, &mix_conv, &mix_res, &tee])?;
    gst::Element::link_many([&mixer, &mix_caps, &mix_conv, &mix_res, &tee])?;
    for e in [&mixer, &mix_caps, &mix_conv, &mix_res, &tee] {
        e.sync_state_with_parent().ok();
    }

    let mut attached = 0usize;

    if !disable_audio && !force_testsrc {
        for spec in specs {
            info!(device = %spec.device, "🔊 audio: attach input start");
            if let Err(e) = attach_audio_input(&pipeline, &mixer, &spec) {
                tracing::warn!(device=%spec.device, ?e, "audio: failed to attach input");
                continue;
            }
            attached += 1;
            info!(device = %spec.device, "🔊 audio: attach input ok");
        }
    }

    if attached == 0 {
        // Fallback: Stille – bzw. mit ORS_AUDIO_TESTSRC=1 ein leiser Sinuston, um die
        // Tonstrecke (Zuschauer, Aufnahme, Verteiler, Lautstärkeregler) ohne Mikrofon zu prüfen
        if force_testsrc {
            info!("audio: ORS_AUDIO_TESTSRC gesetzt → Testton (440 Hz)");
        } else {
            tracing::warn!("audio: no usable inputs, fallback to silence");
        }
        let testsrc = gst::ElementFactory::make("audiotestsrc")
            .name("shared_audiotestsrc")
            .property("is-live", true)
            .property_from_str("wave", if force_testsrc { "sine" } else { "silence" })
            .property("volume", 0.3f64)
            .build()
            .map_err(|e| anyhow!("no audio src found (audiotestsrc missing: {e})"))?;
        let capf = make("capsfilter", "shared_silence_caps")?;
        capf.set_property(
            "caps",
            &gst::Caps::builder("audio/x-raw")
                .field("format", &"S16LE")
                .field("rate", &48_000i32)
                .field("channels", &2i32)
                .build(),
        );
        let conv = make("audioconvert", "shared_silence_conv")?;
        let res = make("audioresample", "shared_silence_res")?;
        pipeline.add_many([&testsrc, &capf, &conv, &res])?;
        gst::Element::link_many([&testsrc, &capf, &conv, &res])?;
        if let (Some(srcpad), Some(sinkpad)) =
            (res.static_pad("src"), mixer.request_pad_simple("sink_%u"))
        {
            srcpad.link(&sinkpad)?;
        }
        for e in [&testsrc, &capf, &conv, &res] {
            e.sync_state_with_parent().ok();
        }
    }

    Ok(tee)
}

/// Prüft grob, ob eine ALSA-Capture-Device-Bezeichnung existiert (z.B. hw:0,0 oder plughw:0,0).
/// Wenn Parsing fehlschlägt, lassen wir es zu (true), um nicht zu strikt zu sein.
fn alsa_capture_device_exists(dev: &str) -> bool {
    // Default immer zulassen
    let lower = dev.to_ascii_lowercase();
    if lower == "default" {
        return true;
    }

    // Erwarte hw:<card>,<device> oder plughw:<card>,<device>
    let dev_part = if let Some(rest) = lower.strip_prefix("hw:") {
        rest
    } else if let Some(rest) = lower.strip_prefix("plughw:") {
        rest
    } else {
        // Unbekanntes Format: nicht blocken
        return true;
    };

    let mut split = dev_part.split(',');
    let card_str = split.next().unwrap_or_default().trim();
    let dev_str = split.next().unwrap_or_default().trim();

    let card_num: u32 = match card_str.parse() {
        Ok(n) => n,
        Err(_) => return true, // nicht blocken, wenn Parsing scheitert
    };
    let dev_num: u32 = match dev_str.parse() {
        Ok(n) => n,
        Err(_) => return true,
    };

    // Prüfe /proc/asound/cardN und /dev/snd/pcmCN*DNd
    let card_path = format!("/proc/asound/card{card_num}");
    if std::path::Path::new(&card_path).exists() {
        // capture device
        let pcm_path = format!("/dev/snd/pcmC{card_num}D{dev_num}c");
        if std::path::Path::new(&pcm_path).exists() {
            return true;
        }
    }

    false
}

/// Versucht, eine ALSA-Device-Angabe aufzulösen.
/// - "hw:X,Y" / "plughw:X,Y" / "default" -> unverändert
/// - Kartenname (z.B. "USB Audio", "[PCH]") -> "hw:<card>,0" wenn gefunden
pub(crate) fn resolve_alsa_device(dev: &str) -> Option<String> {
    let trimmed = dev.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == "default" {
        return Some("default".into());
    }
    // hw:<card>[,<dev>] oder plughw:<card>[,<dev>]
    if lower.starts_with("hw:") || lower.starts_with("plughw:") {
        // fehlende Device-Nummer -> ,0 anhängen
        let with_dev = if trimmed.contains(',') {
            trimmed.to_string()
        } else {
            format!("{},0", trimmed)
        };
        // Immer plughw verwenden, damit der Plug-Layer Format/Raten-Konvertierung übernimmt
        let plug = if with_dev.starts_with("plughw:") {
            with_dev
        } else {
            with_dev.replacen("hw:", "plughw:", 1)
        };
        return Some(plug);
    }
    // reine Zahl → hw:<card>,0
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Some(format!("plughw:{},0", trimmed));
    }

    // Kartenliste aus /proc/asound/cards holen
    let cards = list_alsa_cards_summary();
    for (card_idx, name) in cards {
        let name_lower = name.to_ascii_lowercase();
        if name_lower.contains(&lower) || lower.contains(&name_lower) {
            return Some(format!("plughw:{card_idx},0"));
        }
    }

    None
}

/// Normalisiert eine Audio-Config (JSON-Liste oder CSV) auf stabile ALSA-Pfade (hw:card,device).
/// Gibt None zurück, wenn der String leer ist.
pub(crate) fn normalize_audio_device_config(raw: Option<&str>) -> Option<String> {
    let Some(val) = raw else {
        return None;
    };
    let trimmed = val.trim();
    if trimmed.is_empty() {
        return None;
    }

    // JSON-Liste von AudioInputSpec? → Geräte normalisieren und wieder serialisieren
    if trimmed.starts_with('[') {
        if let Ok(mut list) = serde_json::from_str::<Vec<AudioInputSpec>>(trimmed) {
            for item in list.iter_mut() {
                if let Some(resolved) = resolve_alsa_device(&item.device) {
                    item.device = resolved;
                } else {
                    item.device = item.device.trim().to_string();
                }
            }
            if let Ok(json) = serde_json::to_string(&list) {
                return Some(json);
            }
        }
    }

    // CSV/semicolon-Liste -> jede Komponente normalisieren
    let devices: Vec<String> = trimmed
        .split(';') // Semikolon als Trennzeichen, Kommas bleiben für hw:<card>,<dev> erhalten
        .filter_map(|part| {
            let p = part.trim();
            if p.is_empty() {
                None
            } else {
                Some(resolve_alsa_device(p).unwrap_or_else(|| p.to_string()))
            }
        })
        .collect();

    if devices.is_empty() {
        None
    } else {
        Some(devices.join(";"))
    }
}

/// Liefert (card_idx, name) aus /proc/asound/cards
fn list_alsa_cards_summary() -> Vec<(u32, String)> {
    let mut res = Vec::new();
    let Ok(txt) = std::fs::read_to_string("/proc/asound/cards") else {
        return res;
    };

    for line in txt.lines() {
        let line = line.trim_start();
        let mut parts = line.splitn(2, ' ');
        let card_num_str = parts.next().unwrap_or_default();
        let rest = parts.next().unwrap_or_default();
        let card_num: u32 = match card_num_str.parse() {
            Ok(n) => n,
            Err(_) => continue,
        };

        let name = if let Some(start) = rest.find('[') {
            if let Some(end) = rest[start + 1..].find(']') {
                rest[start + 1..start + 1 + end].trim().to_string()
            } else {
                rest.trim().to_string()
            }
        } else {
            rest.trim().to_string()
        };

        res.push((
            card_num,
            if name.is_empty() {
                format!("card{card_num}")
            } else {
                name
            },
        ));
    }

    res
}

fn parse_audio_inputs(dev_opt: Option<&str>) -> Vec<AudioInputSpec> {
    let mut out: Vec<AudioInputSpec> = Vec::new();
    let Some(raw) = dev_opt else {
        return out;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return out;
    }

    if trimmed.starts_with('[') {
        if let Ok(list) = serde_json::from_str::<Vec<AudioInputSpec>>(trimmed) {
            return list;
        } else {
            tracing::warn!(
                "audio: failed to parse audio device JSON, falling back to single device"
            );
        }
    }

    // Semikolon-separiert (Kommas bleiben Teil von hw:<card>,<device>)
    let parts: Vec<&str> = if trimmed.contains(';') {
        trimmed.split(';').collect()
    } else if !trimmed.to_ascii_lowercase().starts_with("hw:")
        && !trimmed.to_ascii_lowercase().starts_with("plughw:")
        && trimmed.contains(',')
    {
        // ältere Konfigurationen mit Komma als Trenner ohne hw:-Präfix
        trimmed.split(',').collect()
    } else {
        vec![trimmed]
    };

    for dev in parts {
        let d = dev.trim();
        if d.is_empty() {
            continue;
        }
        out.push(AudioInputSpec {
            device: d.to_string(),
            label: None,
            gain: None,
            channels: None,
        });
    }
    out
}

/// Liefert (card, device) aus `hw:X,Y` / `plughw:X,Y` (ohne Device → 0).
fn alsa_card_dev(dev: &str) -> Option<(u32, u32)> {
    let lower = dev.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix("plughw:")
        .or_else(|| lower.strip_prefix("hw:"))?;
    // Namensform hw:CARD=xyz nicht unterstützt (nur Zahlen)
    let mut it = rest.split(',');
    let card: u32 = it.next()?.trim().parse().ok()?;
    let devn: u32 = it.next().map(|d| d.trim().parse().unwrap_or(0)).unwrap_or(0);
    Some((card, devn))
}

/// Kanalzahl aus /proc/asound/card<N>/stream0 (USB-Audio-Geräte listen dort ihre Capture-Formate).
fn usb_stream_capture_channels(card: u32) -> Option<u32> {
    let txt = std::fs::read_to_string(format!("/proc/asound/card{card}/stream0")).ok()?;
    let mut in_capture = false;
    let mut best: Option<u32> = None;
    for line in txt.lines() {
        let t = line.trim();
        if t.starts_with("Capture:") {
            in_capture = true;
            continue;
        }
        if t.starts_with("Playback:") {
            in_capture = false;
            continue;
        }
        if in_capture {
            if let Some(v) = t.strip_prefix("Channels:") {
                if let Ok(n) = v.trim().parse::<u32>() {
                    best = Some(best.map_or(n, |b| b.max(n)));
                }
            }
        }
    }
    best
}

/// Ermittelt die native Capture-Kanalzahl eines ALSA-Geräts: erst über die vom Treiber
/// gemeldeten Caps (`hw:` im READY-Zustand), sonst über /proc/asound (USB-Geräte).
/// `None`, wenn das Gerät nicht adressierbar ist (z.B. „default“).
pub fn detect_capture_channels(device: &str) -> Option<u32> {
    let (card, devn) = alsa_card_dev(device)?;
    let _ = gst::init();
    let mut best: Option<u32> = None;
    if let Ok(src) = gst::ElementFactory::make("alsasrc").name("probe_channels").build() {
        src.set_property("device", format!("hw:{card},{devn}"));
        if src.set_state(gst::State::Ready).is_ok() {
            if let Some(pad) = src.static_pad("src") {
                let caps = pad.query_caps(None);
                for st in caps.iter() {
                    if let Ok(n) = st.get::<i32>("channels") {
                        best = Some(best.map_or(n as u32, |b| b.max(n as u32)));
                    } else if let Ok(r) = st.get::<gst::IntRange<i32>>("channels") {
                        let n = r.max() as u32;
                        best = Some(best.map_or(n, |b| b.max(n)));
                    }
                }
            }
        }
        let _ = src.set_state(gst::State::Null);
    }
    if best.is_none() {
        best = usb_stream_capture_channels(card);
    }
    best.filter(|n| *n >= 1 && *n <= 64)
}

/// Mix-Matrix (2 Ausgangskanäle × `n_in` Eingangskanäle) für `audioconvert`.
/// `channels` sind 1-basierte Hardware-Kanäle; leer = alle Kanäle auf beide Seiten.
fn build_mix_matrix(n_in: u32, channels: &[u32]) -> gst::Array {
    let n = n_in.max(1) as usize;
    let sel: Vec<usize> = channels
        .iter()
        .filter(|c| **c >= 1 && (**c as usize) <= n)
        .map(|c| (*c - 1) as usize)
        .collect();
    let mut left = vec![0f32; n];
    let mut right = vec![0f32; n];
    if sel.len() == 2 {
        left[sel[0]] = 1.0;
        right[sel[1]] = 1.0;
    } else {
        let used: Vec<usize> = if sel.is_empty() { (0..n).collect() } else { sel.clone() };
        let w = 1.0f32 / (used.len().max(1) as f32).sqrt();
        for i in used {
            left[i] = w;
            right[i] = w;
        }
    }
    let row = |v: Vec<f32>| gst::Array::new(v.into_iter().map(|x| x.to_send_value()));
    gst::Array::new([row(left).to_send_value(), row(right).to_send_value()])
}

fn attach_audio_input(
    pipeline: &gst::Pipeline,
    mixer: &gst::Element,
    spec: &AudioInputSpec,
) -> Result<()> {
    let resolved = resolve_alsa_device(&spec.device).unwrap_or_else(|| spec.device.clone());
    if !alsa_capture_device_exists(&resolved) {
        return Err(anyhow!("alsa device not found: {}", resolved));
    }

    // Native Kanalzahl bestimmen. Mehrkanal-Interfaces (z.B. Scarlett 18i20) liefern nur ihre
    // volle Kanalzahl; über plughw auf Stereo gezwungen kämen sonst nur Kanal 1+2 an.
    let native = detect_capture_channels(&resolved);
    let wanted: Vec<u32> = spec.channels.clone().unwrap_or_default();
    let (src_device, in_channels) = match (native, alsa_card_dev(&resolved)) {
        (Some(n), Some((card, devn))) => (format!("hw:{card},{devn}"), Some(n)),
        _ => (resolved.clone(), None),
    };
    info!(
        device_raw = %spec.device,
        device = %src_device,
        native_channels = ?in_channels,
        selected = ?wanted,
        gain = spec.gain.unwrap_or(1.0),
        "🔊 audio: attach input (resolved)"
    );

    let safe_name = resolved.replace([':', ',', ' ', '/'], "_");
    let src = gst::ElementFactory::make("alsasrc")
        .name(&format!("alsasrc_{safe_name}"))
        .build()
        .map_err(|e| anyhow!("missing element: alsasrc ({e})"))?;
    if src.has_property("device", None) {
        src.set_property("device", src_device.as_str());
    }
    if src.has_property("is-live", None) {
        src.set_property("is-live", true);
    }
    let capf_in = make("capsfilter", &format!("acaps_in_{safe_name}"))?;
    let conv = make("audioconvert", &format!("aconv_{safe_name}"))?;
    let res = make("audioresample", &format!("ares_{safe_name}"))?;
    let capf = make("capsfilter", &format!("acaps_{safe_name}"))?;
    capf.set_property(
        "caps",
        &gst::Caps::builder("audio/x-raw")
            .field("format", &"S16LE")
            .field("rate", &48_000i32)
            .field("channels", &2i32)
            .build(),
    );
    match in_channels {
        Some(n) => {
            // Gerät mit voller Kanalzahl öffnen, danach per Matrix auf Stereo mischen
            capf_in.set_property(
                "caps",
                &gst::Caps::builder("audio/x-raw").field("channels", &(n as i32)).build(),
            );
            conv.set_property("mix-matrix", build_mix_matrix(n, &wanted));
        }
        None => {
            // Unbekannte Kanalzahl (z.B. „default“): bisheriges Verhalten, Plug-Layer liefert Stereo
            capf_in.set_property("caps", &gst::Caps::builder("audio/x-raw").build());
        }
    }
    let vol = make("volume", &format!("avol_{safe_name}"))?;
    vol.set_property("volume", spec.gain.unwrap_or(1.0));
    vol.set_property("mute", false);

    pipeline.add_many([&src, &capf_in, &conv, &res, &capf, &vol])?;
    // Quelle -> Eingangs-Caps (native Kanäle) -> convert (Mix-Matrix) -> resample -> 48k/2ch/S16 -> volume
    gst::Element::link_many([&src, &capf_in, &conv, &res, &capf, &vol])?;
    if let (Some(srcpad), Some(sinkpad)) =
        (vol.static_pad("src"), mixer.request_pad_simple("sink_%u"))
    {
        srcpad.link(&sinkpad)?;
    }
    // Probe: Gerät öffnen (Ready + Paused); bei Fehler aufräumen
    let ready_res = src.set_state(gst::State::Ready);
    let paused_res = ready_res
        .as_ref()
        .ok()
        .and_then(|_| src.set_state(gst::State::Paused).ok());
    if ready_res.is_err() || paused_res.is_none() {
        warn!(
            device = %src_device,
            ready_ok = ready_res.is_ok(),
            paused_ok = paused_res.is_some(),
            "🔊 audio: device failed to init"
        );
        let _ = src.set_state(gst::State::Null);
        for e in [&src, &capf_in, &conv, &res, &capf, &vol] {
            pipeline.remove(e).ok();
        }
        return Err(anyhow!("alsa device not ready: {}", src_device));
    }
    let _ = src.set_state(gst::State::Null);

    for e in [&src, &capf_in, &capf, &conv, &res, &vol] {
        e.sync_state_with_parent().ok();
    }

    info!(
        device_raw=%spec.device,
        device=%src_device,
        native_channels=?in_channels,
        selected=?wanted,
        gain=spec.gain.unwrap_or(1.0),
        "audio: attached input"
    );
    Ok(())
}

fn auto_tile(i: usize, n: usize) -> SlotGeom {
    // 1920x1080 Ziel (FullHD)
    let (w, h) = (1920, 1080);

    // einfache Heuristik: 1→1x1, 2→1x2, 3-4→2x2, 5-6→2x3
    let (rows, cols) = match n {
        0 | 1 => (1, 1),
        2 => (1, 2),
        3 | 4 => (2, 2),
        _ => (2, 3),
    };

    let cw = w / cols as i32;
    let ch = h / rows as i32;

    let r = (i / cols) as i32;
    let c = (i % cols) as i32;

    SlotGeom {
        x: c * cw,
        y: r * ch,
        w: cw,
        h: ch,
        z: i as i32,
    }
}

/// alle Inputs + compositor
/// alle Inputs + compositor
fn build_inputs(
    pipeline: &gst::Pipeline,
    cameras: &[SourceSpec],
    slots_needed: usize, // NEU
) -> Result<gst::Element> {
    let comp = make("compositor", "comp")?;
    pipeline.add(&comp)?;

    // Hintergrund bleibt auf dem compositor-Default (schwarz), kein Override notwendig

    let n = cameras.len().min(slots_needed.max(1));
    info!(slots_needed, inputs = cameras.len(), used = n, "🎥 build_inputs");

    for (i, spec) in cameras.iter().take(n).enumerate() {
        let label = spec
            .placeholder_label
            .clone()
            .or_else(|| spec.name.clone())
            .unwrap_or_else(|| format!("Slot {i} – keine Quelle"));

        info!(
            slot = i,
            name = spec.name.as_deref().unwrap_or("-"),
            uri = spec.uri.as_deref().unwrap_or("-"),
            dev = spec.dev.as_deref().unwrap_or("-"),
            is_live = spec.is_live,
            is_placeholder = spec.is_placeholder,
            "🎥 input slot"
        );

        let bin = match build_source_bin(spec, i as i32, label.clone()) {
            Ok(b) => b,
            Err(e) => {
                warn!(
                    "Quelle {} nicht verfügbar ({e:?}) → Fallback Schwarzbild",
                    i
                );
                build_fallback_bin(i as i32, label)?
            }
        };
        pipeline.add(&bin)?;
        bin.sync_state_with_parent()?; // wichtig: Pads existieren sofort

        let srcpad = bin
            .static_pad("src")
            .ok_or_else(|| anyhow!("srcbin src pad"))?;
        let sink = comp
            .request_pad_simple("sink_%u")
            .ok_or_else(|| anyhow!("compositor sink pad"))?;

        // Auto-Grid fürs Erstlayout – spätere apply_layout überschreibt das
        let g = auto_tile(i, n);
        sink.set_property("xpos", g.x);
        sink.set_property("ypos", g.y);
        sink.set_property("width", g.w);
        sink.set_property("height", g.h);
        sink.set_property("zorder", g.z.max(0) as u32);
        sink.set_property("alpha", 1.0f64);

        srcpad.link(&sink)?;
        info!(slot = i, pad = %sink.name(), "🎥 input linked");
    }

    Ok(comp.upcast())
}

fn build_fallback_bin(index: i32, label: String) -> Result<gst::Bin> {
    let to_comp_caps = caps_to_compositor();
    let bin = gst::Bin::new();
    bin.set_property("name", &format!("srcbin_fb_only_{index}"));

    let fb_src = gst::ElementFactory::make("videotestsrc")
        .property("is-live", true)
        .property_from_str("pattern", "black")
        .build()?;
    let fb_conv = make("videoconvert", &format!("fb_only_conv_{index}"))?;
    let fb_text = make("textoverlay", &format!("fb_only_text_{index}"))?;
    fb_text.set_property("text", &label);
    fb_text.set_property_from_str("halignment", "center");
    fb_text.set_property_from_str("valignment", "top");
    fb_text.set_property("draw-shadow", true);
    fb_text.set_property("font-desc", "Sans 36");
    let fb_cf = make("capsfilter", &format!("fb_only_cf_{index}"))?;
    fb_cf.set_property("caps", &to_comp_caps);

    bin.add_many([&fb_src, &fb_conv, &fb_text, &fb_cf])?;
    gst::Element::link_many([&fb_src, &fb_conv, &fb_text, &fb_cf])?;

    let srcpad = fb_cf
        .static_pad("src")
        .ok_or_else(|| anyhow!("fb_only cf src pad"))?;
    let ghost = gst::GhostPad::with_target(&srcpad).map_err(|_| anyhow!("ghost pad"))?;
    ghost.set_property("name", &"src");
    bin.add_pad(&ghost).map_err(|_| anyhow!("ghost pad"))?;

    Ok(bin)
}

/// =================================================
/// 2) Mixer-Output
/// =================================================
fn build_mixer_output(pipeline: &gst::Pipeline, comp: &gst::Element) -> Result<gst::Element> {
    let vconv = make("videoconvert", "mix_conv")?;
    let vscale = make("videoscale", "mix_scale")?;

    let capf_rgba = make("capsfilter", "mix_caps")?;
    let out_caps = gst::Caps::builder("video/x-raw")
        .field("format", &"ARGB") // explizit!
        .field("width", &1920i32)
        .field("height", &1080i32)
        .field("framerate", &gst::Fraction::new(30, 1))
        .build();
    capf_rgba.set_property("caps", &out_caps);

    pipeline.add_many([&vconv, &vscale, &capf_rgba])?;
    gst::Element::link_many([comp, &vconv, &vscale, &capf_rgba])?;
    Ok(capf_rgba.upcast())
}

/// =================================================
/// 3) Encoder + Tee
/// =================================================
fn build_encoder_and_tee(pipeline: &gst::Pipeline, input: &gst::Element) -> Result<gst::Element> {
    // Zusatz: explizit vor dem Encoder auf I420 konvertieren
    let conv_enc = make("videoconvert", "enc_conv")?;
    let capf_i420 = make("capsfilter", "enc_caps")?;
    let i420_caps = gst::Caps::builder("video/x-raw")
        .field("format", &"I420")
        .build();
    capf_i420.set_property("caps", &i420_caps);

    let x264 = gst::ElementFactory::make("x264enc")
        .name("enc")
        .property_from_str("tune", "zerolatency")
        .property_from_str("speed-preset", "veryfast")
        .property("key-int-max", 30u32)
        .property("byte-stream", false)
        .property("threads", 2u32)
        .build()
        .map_err(|_| anyhow!("missing element: x264enc"))?;

    let tee = make("tee", "tee_enc")?;

    pipeline.add_many([&conv_enc, &capf_i420, &x264, &tee])?;

    // Wichtig: input (RGBA) → conv_enc → enc_caps(I420) → x264 → tee
    gst::Element::link_many([input, &conv_enc, &capf_i420, &x264, &tee])
        .map_err(|_| anyhow!("failed to link mix → conv_enc → enc_caps → enc → tee"))?;

    Ok(tee)
}

/// =================================================
/// 4) Outputs
/// =================================================

fn build_record_output(
    pipeline: &gst::Pipeline,
    tee: &gst::Element,
    rec: &RecordOptions,
) -> Result<()> {
    let _ = std::fs::create_dir_all(&rec.directory);

    // --- Splitmuxsink: Container + Location wie bisher ---
    let split_rec = make("splitmuxsink", "rec_split")?;
    match rec.container.as_str() {
        "mp4" => split_rec.set_property_from_str("muxer-factory", "mp4mux"),
        "mkv" => split_rec.set_property_from_str("muxer-factory", "matroskamux"),
        //"mov" => split_rec.set_property_from_str("qtmux"),
        _ => split_rec.set_property_from_str("muxer-factory", "matroskamux"),
    }
    let ext = if rec.container == "mp4" {
        "mp4"
    } else if rec.container == "mov" {
        "mov"
    } else {
        "mkv"
    };
    split_rec.set_property(
        "location",
        &format!("{}/{}_rec_%05d.{}", rec.directory, rec.basename, ext),
    );
    split_rec.set_property("max-size-time", (rec.split_ms.max(0) as u64) * 1_000_000u64);
    split_rec.set_property("max-files", rec.max_files.max(0) as u32);
    split_rec.set_property("send-keyframe-requests", true);

    // --- VIDEO: tee → queue → h264parse → splitmuxsink.video ---
    let q_rec = make_queue("q_rec_video")?;
    let parse_r = make("h264parse", "parse_rec")?;
    parse_r.set_property("config-interval", -1i32);

    // wichtig: alle Elemente im Pipeline-Graph haben
    pipeline.add_many([&q_rec, &parse_r, &split_rec])?;

    // tee → q_rec
    let tee_src = tee
        .request_pad_simple("src_%u")
        .ok_or_else(|| anyhow!("tee pad"))?;
    let q_sink = q_rec
        .static_pad("sink")
        .ok_or_else(|| anyhow!("queue sink pad"))?;
    tee_src.link(&q_sink)?;

    // q_rec → parse_r
    gst::Element::link_many([&q_rec, &parse_r])?;

    // parse_r → splitmuxsink.video
    let video_pad = split_rec
        .request_pad_simple("video")
        .ok_or_else(|| anyhow!("splitmuxsink video pad"))?;
    let parse_src = parse_r
        .static_pad("src")
        .ok_or_else(|| anyhow!("parse_rec src pad"))?;
    parse_src.link(&video_pad)?;

    // Keep for config compatibility even if current recording path uses shared audio tee.
    let _ = rec.audio_dev.as_deref();

    if let Err(e) = attach_record_audio(pipeline, &split_rec) {
        warn!(error = ?e, "Recording audio disabled, video-only recording");
    }

    info!(
        dir=%rec.directory,
        base=%rec.basename,
        seg_ms=rec.split_ms,
        max_files=rec.max_files,
        fmt=%rec.container,
        "⏺️ Recording branch enabled (video + audio, sofern verfügbar)"
    );
    Ok(())
}

fn attach_record_audio(
    pipeline: &gst::Pipeline,
    split_rec: &gst::Element,
) -> Result<()> {
    // Statt eigener Quelle nutzen wir die globale Audio-Quelle (Tee)
    let tee = pipeline
        .by_name("shared_audio_tee")
        .ok_or_else(|| anyhow!("shared audio source not found"))?;

    // Rest der Kette
    let aconv = make("audioconvert", "rec_aconv")?;
    let ares = make("audioresample", "rec_ares")?;
    let vol = make("volume", "rec_volume")?;
    vol.set_property("volume", 1.0);
    vol.set_property("mute", false);
    let q_a = make_queue("q_rec_audio")?;
    let aenc = gst::ElementFactory::make("avenc_aac")
        .name("rec_aac")
        .property("bitrate", 128_000i32)
        .build()
        .map_err(|_| anyhow!("missing element: avenc_aac (install gst-libav)"))?;
    let aparse = make("aacparse", "rec_aacparse")?;

    // Alles zur Pipeline hinzufügen
    pipeline.add_many([&aconv, &ares, &vol, &q_a, &aenc, &aparse])?;

    // shared tee → aconv → ares → vol → q_a → aenc → aparse
    gst::Element::link_many([&aconv, &ares, &vol, &q_a, &aenc, &aparse])?;
    if let Some(pad) = tee.request_pad_simple("src_%u") {
        let sink = aconv
            .static_pad("sink")
            .ok_or_else(|| anyhow!("rec_aconv sink pad"))?;
        pad.link(&sink)?;
    } else {
        return Err(anyhow!("failed to request pad from shared_audio_tee"));
    }

    // q_a.src → splitmuxsink.audio_%u
    let q_src = aparse
        .static_pad("src")
        .ok_or_else(|| anyhow!("rec audio parse src pad"))?;

    let a_sink = split_rec
        .request_pad_simple("audio_%u")
        .ok_or_else(|| anyhow!("rec mux audio sink pad"))?;

    q_src.link(&a_sink)?;

    Ok(())
}

/// Zusätzlicher Stats-Zweig: zählt Frames und berechnet grobe FPS.
fn attach_stats_branch(
    pipeline: &gst::Pipeline,
    tee: &gst::Element,
    metrics: Arc<Mutex<PipelineMetrics>>,
) -> Result<()> {
    let q_stats = make_queue("q_stats")?;
    let sink = make("fakesink", "stats_fakesink")?;
    sink.set_property("sync", false);
    sink.set_property("async", false);

    pipeline.add_many([&q_stats, &sink])?;

    let tee_src = tee
        .request_pad_simple("src_%u")
        .ok_or_else(|| anyhow!("tee pad for stats"))?;
    let q_sink = q_stats
        .static_pad("sink")
        .ok_or_else(|| anyhow!("q_stats sink pad"))?;
    tee_src.link(&q_sink)?;

    gst::Element::link_many([&q_stats, &sink])?;

    // Pad-Probe zum Zählen der Frames
    if let Some(pad) = q_stats.static_pad("sink") {
        let metrics_clone = metrics.clone();
        pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
            if info.data.is_some() {
                let now = Instant::now();
                let mut m = metrics_clone.lock().unwrap();
                m.frame_count += 1;
                if let Some(gst::PadProbeData::Buffer(ref buf)) = info.data.as_ref() {
                    if buf.flags().contains(gst::BufferFlags::DISCONT) {
                        m.dropped = m.dropped.saturating_add(1);
                    }
                }
                let dt = now.duration_since(m.last_ts);
                if dt >= Duration::from_millis(900) {
                    m.fps = (m.frame_count as f64) / (dt.as_secs_f64().max(0.001));
                    m.frame_count = 0;
                    m.last_ts = now;
                }
            }
            gst::PadProbeReturn::Ok
        });
    }

    // States übernehmen
    q_stats.sync_state_with_parent()?;
    sink.sync_state_with_parent()?;

    Ok(())
}

/// =================================================
/// 5) Alles zusammen
/// =================================================
pub fn build_pipeline(
    mut cameras: Vec<SourceSpec>,
    rec_opt_external: Option<RecordOptions>,
    out_rtmp: Option<&str>,
    layout_slots: usize, // NEU
    audio_dev: Option<&str>,
    metrics: Arc<Mutex<PipelineMetrics>>,
) -> Result<gst::Pipeline> {
    let _ = gst::init();
    let pipeline = gst::Pipeline::new();

    // Eine gemeinsame Audio-Quelle anlegen, damit alle Äste dieselbe Quelle nutzen
    info!(
        layout_slots,
        camera_count = cameras.len(),
        audio_dev = audio_dev.unwrap_or("-"),
        "🧱 build_pipeline: config"
    );
    let _ = build_shared_audio_src(&pipeline, audio_dev);

    // Wie viele Slots brauchen wir wirklich?
    let slots_needed = layout_slots
        .max(1) // mindestens 1 Slot
        .min(MAX_SOURCES);
    info!(slots_needed, "🧱 build_pipeline: slots determined");

    // Wenn keine Kameras konfiguriert sind: eine Dummy-Kamera (Testbild)
    if cameras.is_empty() {
        cameras.push(SourceSpec {
            name: Some("Testbild".into()),
            ..Default::default()
        });
    }

    // Falls wir weniger Kameras als Slots haben → mit Dummy füllen
    while cameras.len() < slots_needed {
        let idx = cameras.len();
        cameras.push(SourceSpec {
            name: Some(format!("Slot {idx} – Testbild")),
            ..Default::default()
        });
    }

    // Falls mehr Kameras als Slots → abschneiden
    if cameras.len() > slots_needed {
        cameras.truncate(slots_needed);
    }
    info!(
        camera_count = cameras.len(),
        slots_needed,
        "🧱 build_pipeline: sources prepared"
    );

    // 1) Inputs
    let comp = build_inputs(&pipeline, &cameras, slots_needed)?;
    // 2) Mixer output
    let mix_out = build_mixer_output(&pipeline, &comp)?;
    // 3) Encoder + tee
    let tee = build_encoder_and_tee(&pipeline, &mix_out)?;

    // 4) Outputs – nur WebRTC + Recording (TCP/Audience deaktiviert)

    if let Some(rec) = &rec_opt_external {
        if rec.enabled {
            build_record_output(&pipeline, &tee, rec)?;
        }
    } else {
        debug!("Recording disabled (no rec_opt_external)");
    }

    // Kein fest eingebauter WebRTC-Ausgang mehr: jeder Zuschauer bekommt in ws.rs einen
    // eigenen Zweig. Der frühere feste Zweig (webrtcbin ohne Gegenstelle) blieb nach dem
    // ersten Bild hängen; sobald ein Zuschauer einen neuen Zweig an tee_enc hängte, fror
    // dadurch der gesamte Video-Abzweig ein (Aufnahme, Verteiler, alle Zuschauer).

    match out_rtmp.map(str::trim).filter(|u| !u.is_empty()) {
        Some(url) => {
            info!(url = %mask_stream_key(url), "📡 RTMP output enabled (building branch)");
            build_rtmp_output(&pipeline, &tee, url)?;
        }
        None => set_distributor_disabled(),
    }
    // Audience-Stream wird nun lazy nach dem erfolgreichen Start angehängt (siehe PipelineManager::attach_audience_if_enabled)
    // Stats-Zweig (FPS)
    attach_stats_branch(&pipeline, &tee, metrics)?;

    Ok(pipeline)
}

fn build_rtmp_output(pipeline: &gst::Pipeline, tee: &gst::Element, url: &str) -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};

    let q_rtmp = make_queue("q_rtmp")?;
    let parse = make("h264parse", "rtmp_parse")?;
    parse.set_property("config-interval", 1i32);

    let flv = make("flvmux", "rtmp_mux")?;
    flv.set_property("streamable", true);

    let sink = make("rtmpsink", "rtmp_sink")?;
    sink.set_property("location", url);
    sink.set_property("sync", false);

    pipeline.add_many([&q_rtmp, &parse, &flv, &sink])?;
    gst::Element::link_many([&q_rtmp, &parse, &flv, &sink])
        .map_err(|_| anyhow!("failed to link RTMP branch"))?;

    // Ton: gemeinsame Audioquelle (Roh-Audio) → AAC → flvmux. FLV kennt für AAC nur
    // 44,1 kHz (bzw. Teiler davon), daher wird umgetastet.
    let audio_tee = pipeline
        .by_name("shared_audio_tee")
        .ok_or_else(|| anyhow!("shared audio source not found"))?;
    let q_a = make_queue("rtmp_aq")?;
    let aconv = make("audioconvert", "rtmp_aconv")?;
    let ares = make("audioresample", "rtmp_ares")?;
    let acaps = make("capsfilter", "rtmp_acaps")?;
    acaps.set_property(
        "caps",
        &gst::Caps::builder("audio/x-raw")
            .field("rate", &44_100i32)
            .field("channels", &2i32)
            .build(),
    );
    let aenc = gst::ElementFactory::make("avenc_aac")
        .name("rtmp_aac")
        .property("bitrate", 128_000i32)
        .build()
        .map_err(|_| anyhow!("missing element: avenc_aac (install gst-libav)"))?;
    let aparse = make("aacparse", "rtmp_aacparse")?;
    pipeline.add_many([&q_a, &aconv, &ares, &acaps, &aenc, &aparse])?;
    gst::Element::link_many([&q_a, &aconv, &ares, &acaps, &aenc, &aparse])
        .map_err(|_| anyhow!("failed to link RTMP audio chain"))?;
    let flv_audio = flv
        .request_pad_simple("audio")
        .ok_or_else(|| anyhow!("flvmux audio pad"))?;
    aparse
        .static_pad("src")
        .ok_or_else(|| anyhow!("aacparse src pad"))?
        .link(&flv_audio)?;

    // Ausfallschutz: Ein nicht erreichbarer oder abbrechender Verteiler darf die Übung
    // nicht lahmlegen.
    // 1. Die Zweig-Elemente sind vom Pipeline-Zustand entkoppelt (locked state). Sonst
    //    scheitert beim Start der Zustandswechsel von rtmpsink, die Pipeline bleibt in
    //    PAUSED hängen (keine Aufnahme, kein Livebild) und blockiert später beim Stopp.
    // 2. An tee gehängt wird der Zweig erst, wenn er läuft (eigener Thread, wie bei den
    //    Zuschauer-Zweigen). Ein angehängter, aber gestoppter Zweig lehnt schon die
    //    Sticky-Events (Caps/Segment) ab – tee meldet das als FLUSHING bis zur Quelle.
    // 3. Fällt der Verteiler im Betrieb aus, verwirft ein Probe am tee-Ausgang ab sofort
    //    jeden Puffer mit Rückgabe OK (tee sieht den Flow-Fehler nie), danach wird der
    //    Zweig abgehängt und heruntergefahren.
    // Reihenfolge: Eingänge zuerst, Senke zuletzt (gestartet wird rückwärts)
    let elements = vec![
        q_rtmp.clone(),
        parse.clone(),
        q_a.clone(),
        aconv.clone(),
        ares.clone(),
        acaps.clone(),
        aenc.clone(),
        aparse.clone(),
        flv.clone(),
        sink.clone(),
    ];
    for el in &elements {
        el.set_locked_state(true);
    }
    let weak_elements: Vec<gst::glib::WeakRef<gst::Element>> =
        elements.iter().map(|e| e.downgrade()).collect();

    let failed = Arc::new(AtomicBool::new(false));
    // Angehängte tee-Ausgänge (Bild, Ton) – zum Abhängen bei Ausfall
    let tee_pads: Arc<Mutex<Vec<(gst::Element, gst::Pad)>>> = Arc::new(Mutex::new(Vec::new()));

    let streaming_seen = Arc::new(AtomicBool::new(false));
    let failed_sink = failed.clone();
    if let Some(sink_pad) = sink.static_pad("sink") {
        sink_pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
            if !failed_sink.load(Ordering::Acquire)
                && !streaming_seen.swap(true, Ordering::AcqRel)
            {
                set_distributor_status(|s| {
                    if s.state != "error" {
                        s.state = "streaming".into();
                    }
                });
            }
            gst::PadProbeReturn::Ok
        });
    }

    let mark_failed = {
        let failed = failed.clone();
        let tee_pads = tee_pads.clone();
        let weak_elements = weak_elements.clone();
        Arc::new(move |reason: String| {
            if failed.swap(true, Ordering::AcqRel) {
                return;
            }
            warn!(%reason, "📡 Verteiler (RTMP) ausgefallen – Zweig wird verworfen, Übung läuft weiter");
            set_distributor_status(|s| {
                s.state = "error".into();
                s.last_error = Some(reason.clone());
            });
            crate::logging::emit_ui_event(serde_json::json!({
                "type": "distributor_failed",
                "message": reason,
                "ts": chrono::Utc::now().to_rfc3339(),
            }));
            // Abhängen/Herunterfahren nicht im Streaming-Thread, der den Fehler meldet
            let tee_pads = tee_pads.clone();
            let weak_elements = weak_elements.clone();
            std::thread::spawn(move || {
                let pads = std::mem::take(&mut *tee_pads.lock().unwrap());
                for (tee, pad) in pads {
                    if let Some(peer) = pad.peer() {
                        let _ = pad.unlink(&peer);
                    }
                    tee.release_request_pad(&pad);
                }
                for el in weak_elements.iter().filter_map(|w| w.upgrade()) {
                    let _ = el.set_state(gst::State::Null);
                }
            });
        })
    };

    {
        let weak_pipe = pipeline.downgrade();
        let weak_tee = tee.downgrade();
        let weak_audio_tee = audio_tee.downgrade();
        let failed = failed.clone();
        let tee_pads = tee_pads.clone();
        let mark_failed = mark_failed.clone();
        std::thread::spawn(move || {
            // Warten, bis die Pipeline läuft (max. 15 s). Beim Aufruf steht sie noch auf
            // NULL – gestartet wird sie erst, nachdem build_pipeline zurückkehrt.
            let mut playing = false;
            for _ in 0..150 {
                let Some(p) = weak_pipe.upgrade() else { return };
                if p.current_state() == gst::State::Playing {
                    playing = true;
                    break;
                }
                drop(p);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if !playing {
                warn!("📡 RTMP branch not started: pipeline did not reach PLAYING");
                return;
            }
            // Senke zuerst, damit stromabwärts alles bereit ist, wenn Daten kommen
            for el in weak_elements.iter().rev().filter_map(|w| w.upgrade()) {
                if failed.load(Ordering::Acquire) {
                    return;
                }
                if el.set_state(gst::State::Playing).is_err() {
                    mark_failed(format!("{} konnte nicht gestartet werden", el.name()));
                    return;
                }
            }
            // Bild: tee_enc → q_rtmp (elements[0]), Ton: shared_audio_tee → rtmp_aq (elements[2])
            for (weak_t, idx) in [(&weak_tee, 0usize), (&weak_audio_tee, 2usize)] {
                let (Some(t), Some(q)) = (weak_t.upgrade(), weak_elements[idx].upgrade()) else {
                    return;
                };
                let Some(q_sink) = q.static_pad("sink") else { return };
                let Some(src) = t.request_pad_simple("src_%u") else {
                    mark_failed("kein freier tee-Ausgang".into());
                    return;
                };
                crate::ws::drop_upstream_reconfigure(&src);
                let failed_probe = failed.clone();
                src.add_probe(
                    gst::PadProbeType::BUFFER | gst::PadProbeType::BUFFER_LIST,
                    move |_, _| {
                        if failed_probe.load(Ordering::Acquire) {
                            gst::PadProbeReturn::Drop
                        } else {
                            gst::PadProbeReturn::Ok
                        }
                    },
                );
                if src.link(&q_sink).is_err() {
                    t.release_request_pad(&src);
                    mark_failed("Zweig konnte nicht angehängt werden".into());
                    return;
                }
                tee_pads.lock().unwrap().push((t, src));
            }
            info!("📡 RTMP branch running and attached");
        });
    }

    if let Some(bus) = pipeline.bus() {
        bus.set_sync_handler(move |_, msg| {
            if let gst::MessageView::Error(e) = msg.view() {
                let from_branch = msg
                    .src()
                    .map(|s| {
                        let n = s.name();
                        n.starts_with("rtmp_") || n == "q_rtmp"
                    })
                    .unwrap_or(false);
                if from_branch {
                    mark_failed(e.error().to_string());
                    // Folgefehler des Zweigs nicht an den allgemeinen Bus-Watch weitergeben
                    return gst::BusSyncReply::Drop;
                }
            }
            gst::BusSyncReply::Pass
        });
    }

    set_distributor_status(|s| {
        s.enabled = true;
        s.url = mask_stream_key(url);
        s.state = "connecting".into();
        s.last_error = None;
    });
    info!(url = %mask_stream_key(url), "📡 RTMP branch enabled and linked");
    Ok(())
}
