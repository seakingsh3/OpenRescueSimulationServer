//! src/voice.rs – „Voice of Room“
//!
//! Push-to-Talk vom Browser (WebRTC/Opus) → Server → Raumlautsprecher (RTP/Opus über UDP)
//! und optional an Zuschauer-Viewer (WebRTC).
//!
//! Lautsprecher melden sich selbst über `/ws/voice/speaker` an (Registry), damit keine
//! festen IP-Adressen gepflegt werden müssen. Der Sender verbindet sich mit `/ws/voice/tx`.
//! Es gibt höchstens eine aktive Sende-Sitzung; ein neuer Sender beendet die alte sauber.

use crate::state::AppCtx;
use anyhow::{anyhow, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use futures_util::{SinkExt, StreamExt};
use glib::MainContext;
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_sdp as gst_sdp;
use gstreamer_webrtc as gst_webrtc;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};

/// RTP-Payload-Typ für Opus zu den Lautsprechern (dynamischer Bereich, üblich für Opus).
pub const OPUS_PT: i32 = 111;
/// RTP-Payload-Typ für den L16/PCM-Zweig (für Geräte ohne Opus-Decoder, z.B. ESP32).
pub const L16_PT: i32 = 97;
/// Abtastrate des L16-Zweigs. Bewusst niedriger als Opus (48 kHz), weil unkomprimiertes
/// PCM auf schwacher Hardware (ESP32) sonst zu viel Bandbreite/RAM braucht. Für
/// Sprachdurchsagen ausreichend.
pub const L16_RATE: i32 = 16000;
/// Nach dieser Zeit ohne Heartbeat gilt ein Lautsprecher als offline.
const OFFLINE_AFTER: Duration = Duration::from_secs(15);
/// „Empfängt“ = Heartbeat mit playing=true, jünger als diese Zeit.
const RECEIVING_WITHIN: Duration = Duration::from_secs(8);

// ============================================================================
// Lautsprecher-Registry (Laufzeitzustand; Name/Gruppe/Lautstärke liegen in der DB)
// ============================================================================

#[derive(Clone)]
#[allow(dead_code)]
pub struct SpeakerLive {
    pub device_id: String,
    pub db_id: i64,
    pub name: String,
    pub group: String,
    pub ip: IpAddr,
    pub rtp_port: u16,
    pub codec: String,
    pub version: String,
    pub volume: i64,
    pub muted: bool,
    pub playing: bool,
    pub rms_db: f64,
    pub last_seen: Instant,
    pub connected_at: Instant,
    pub last_heartbeat_playing: Option<Instant>,
    /// Freigabe: "approved" (mit Schlüssel), "pending" (wartet auf Freigabe, kein Ton),
    /// "legacy" (alte Firmware ohne Schlüssel-Unterstützung, übergangsweise beschallt)
    pub auth: &'static str,
    tx: mpsc::UnboundedSender<String>,
}

impl SpeakerLive {
    /// Bekommt dieses Gerät Ton? (nicht freigegebene Geräte nicht)
    pub fn may_receive(&self) -> bool {
        self.auth != "pending"
    }
    pub fn is_online(&self) -> bool {
        self.last_seen.elapsed() < OFFLINE_AFTER
    }
    pub fn is_receiving(&self) -> bool {
        self.is_online()
            && self
                .last_heartbeat_playing
                .map(|t| t.elapsed() < RECEIVING_WITHIN)
                .unwrap_or(false)
    }
    fn send(&self, msg: &Value) {
        let _ = self.tx.send(msg.to_string());
    }
}

static SPEAKERS: Lazy<Arc<Mutex<HashMap<String, SpeakerLive>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

pub async fn live_speakers() -> Vec<SpeakerLive> {
    SPEAKERS.lock().await.values().cloned().collect()
}

/// Ziel einer Durchsage: eine Lautsprecher-Gruppe oder die in einem Raum hinterlegten
/// Lautsprecher (Raumgestaltung). Im Browser als `group=<name>` bzw. `group=room:<id>`.
#[derive(Clone, Debug)]
pub enum VoiceTarget {
    Group(String),
    Room { id: i64, name: String, speakers: HashSet<i64> },
}

impl VoiceTarget {
    /// Gehört ein Lautsprecher (Gruppe laut DB, DB-ID) zu diesem Ziel?
    pub fn matches(&self, group: &str, db_id: i64) -> bool {
        match self {
            VoiceTarget::Group(g) => g.eq_ignore_ascii_case(group),
            VoiceTarget::Room { speakers, .. } => speakers.contains(&db_id),
        }
    }
    /// Anzeigename für Status und Logs
    pub fn label(&self) -> String {
        match self {
            VoiceTarget::Group(g) => g.clone(),
            VoiceTarget::Room { name, .. } => format!("Raum {name}"),
        }
    }
}

/// Lautsprecher-IDs aus dem JSON-Feld `rooms.speakers` (z.B. "[3,7]").
pub fn parse_room_speakers(raw: Option<&str>) -> HashSet<i64> {
    raw.and_then(|s| serde_json::from_str::<Vec<i64>>(s).ok())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// `group`-Parameter des Senders in ein Ziel übersetzen. `room:<id>` → Lautsprecher des
/// Raums aus der DB; unbekannter Raum → Fehlermeldung statt stiller Durchsage ins Leere.
async fn resolve_target(ctx: &AppCtx, group: &str) -> Result<VoiceTarget> {
    if let Some(rest) = group.strip_prefix("room:") {
        let id: i64 = rest.trim().parse().map_err(|_| anyhow!("ungültiger Raum: {group}"))?;
        let room = crate::db::room_get(&ctx.db, id)
            .await
            .map_err(|_| anyhow!("Raum #{id} nicht gefunden"))?;
        return Ok(VoiceTarget::Room {
            id,
            speakers: parse_room_speakers(room.speakers.as_deref()),
            name: room.name,
        });
    }
    Ok(VoiceTarget::Group(group.to_string()))
}

async fn online_for(target: &VoiceTarget) -> Vec<SpeakerLive> {
    SPEAKERS
        .lock()
        .await
        .values()
        .filter(|s| s.is_online() && s.may_receive() && target.matches(&s.group, s.db_id))
        .cloned()
        .collect()
}

async fn notify_target(target: &VoiceTarget, msg: &Value) {
    for s in online_for(target).await {
        s.send(msg);
    }
}

async fn notify_device(device_id: &str, msg: &Value) -> bool {
    if let Some(s) = SPEAKERS.lock().await.get(device_id) {
        s.send(msg);
        return true;
    }
    false
}

// ============================================================================
// Sende-Sitzung (eine GStreamer-Pipeline je aktivem Sender)
// ============================================================================

pub struct VoiceViewerBranch {
    pub id: u64,
    pub cleanup: Box<dyn FnOnce() + Send + 'static>,
}

#[derive(Clone)]
pub struct VoiceHandle {
    pub pipeline: gst::Pipeline,
    pub webrtc: gst::Element,
    pub tee: gst::Element,
    pub udpsink: gst::Element,
    /// Zweiter Ausgang: unkomprimiertes L16/PCM für Geräte ohne Opus-Decoder (z.B. ESP32).
    pub l16_udpsink: gst::Element,
    /// Ziel der Durchsage; veränderbar, damit Raum-Änderungen während einer laufenden
    /// Durchsage sofort greifen (siehe `room_speakers_changed`).
    pub target: Arc<std::sync::RwLock<VoiceTarget>>,
    pub viewer_branches: Arc<Mutex<Vec<VoiceViewerBranch>>>,
}

impl VoiceHandle {
    /// Aktuelles Ziel (Kopie)
    pub fn target(&self) -> VoiceTarget {
        self.target.read().unwrap().clone()
    }
}

/// Lautsprecher eines Raums wurden geändert. Läuft gerade eine Durchsage an diesen Raum,
/// werden hinzugekommene Geräte sofort beschallt und entfernte sofort abgeschaltet –
/// statt erst bei der nächsten Durchsage.
pub async fn room_speakers_changed(room_id: i64, speakers: HashSet<i64>) {
    let Some(h) = VOICE_SESSION.lock().await.clone() else { return };
    let old = {
        let mut t = h.target.write().unwrap();
        match &mut *t {
            VoiceTarget::Room { id, speakers: cur, .. } if *id == room_id => std::mem::replace(cur, speakers.clone()),
            _ => return,
        }
    };
    let live = live_speakers().await;
    let (mut added, mut removed) = (0, 0);
    for s in live.iter().filter(|s| s.is_online() && s.may_receive()) {
        let (was, is) = (old.contains(&s.db_id), speakers.contains(&s.db_id));
        if is && !was {
            udp_add(speaker_sink(&h, &s.codec), s.ip, s.rtp_port);
            s.send(&json!({"type":"stream","active":true}));
            added += 1;
        } else if was && !is {
            udp_remove(speaker_sink(&h, &s.codec), s.ip, s.rtp_port);
            s.send(&json!({"type":"stream","active":false}));
            removed += 1;
        }
    }
    tracing::info!(room_id, added, removed, "🔊 Raum-Lautsprecher während laufender Durchsage geändert");
}

/// Wählt je nach gemeldetem Lautsprecher-Codec den passenden UDP-Ausgang der Sitzung.
/// Alles außer "l16" (case-insensitiv) bekommt weiterhin Opus — bestehendes Verhalten
/// für den Pi-Client und alle unbekannten/künftigen Codecs bleibt unverändert.
fn speaker_sink<'a>(h: &'a VoiceHandle, codec: &str) -> &'a gst::Element {
    if codec.eq_ignore_ascii_case("l16") {
        &h.l16_udpsink
    } else {
        &h.udpsink
    }
}

static VOICE_SESSION: Lazy<Arc<Mutex<Option<VoiceHandle>>>> =
    Lazy::new(|| Arc::new(Mutex::new(None)));
static VIEWER_BRANCH_ID: AtomicU64 = AtomicU64::new(1);
/// Laufende Nummer der aktuellen Durchsage (0 = keine). Durchsage-Tickets für Viewer und
/// Pager gelten nur für die Durchsage, für die sie ausgestellt wurden.
static VOICE_NONCE: AtomicU64 = AtomicU64::new(0);
static VOICE_NONCE_NEXT: AtomicU64 = AtomicU64::new(1);

pub fn current_voice_nonce() -> Option<i64> {
    match VOICE_NONCE.load(Ordering::Relaxed) {
        0 => None,
        n => Some(n as i64),
    }
}
/// Läuft gerade eine Durchsage, die auch an Viewer geht? Viewer, die sich währenddessen
/// (neu) verbinden, bekommen dann sofort `paging_start` (siehe routes::viewer_ws_connection).
static VIEWER_PAGING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn viewer_paging_active() -> bool {
    VIEWER_PAGING.load(Ordering::Relaxed)
}

pub async fn stop_voice() {
    let taken = VOICE_SESSION.lock().await.take();
    VOICE_NONCE.store(0, Ordering::Relaxed);
    if let Some(h) = taken {
        {
            let mut branches = h.viewer_branches.lock().await;
            while let Some(b) = branches.pop() {
                (b.cleanup)();
            }
        }
        let p = h.pipeline.clone();
        // NULL im GLib-Kontext, damit der Aufrufer nicht an rtsp/udp-Teardown hängt
        MainContext::default().invoke(move || {
            let _ = p.set_state(gst::State::Null);
        });
        tracing::info!(target = %h.target().label(), "🔇 voice session stopped");
    }
}

fn udp_add(sink: &gst::Element, ip: IpAddr, port: u16) {
    sink.emit_by_name::<()>("add", &[&ip.to_string(), &(port as i32)]);
}
fn udp_remove(sink: &gst::Element, ip: IpAddr, port: u16) {
    sink.emit_by_name::<()>("remove", &[&ip.to_string(), &(port as i32)]);
}

/// Baut die Pipeline: webrtcbin(recv) → depay → opusdec → convert → opusenc → rtppay → tee →
/// multiudpsink (Lautsprecher) [+ Viewer-Zweige]. Eine bestehende Sitzung wird zuvor beendet.
async fn start_voice_session(target: &VoiceTarget) -> Result<VoiceHandle> {
    stop_voice().await;
    let _ = gst::init();

    let pipeline = gst::Pipeline::new();
    let mk = |factory: &str, name: &str| -> Result<gst::Element> {
        gst::ElementFactory::make(factory)
            .name(name)
            .build()
            .map_err(|_| anyhow!("missing element: {factory}"))
    };

    let webrtc = gst::ElementFactory::make("webrtcbin")
        .name("voice_webrtc")
        .property_from_str("bundle-policy", "max-bundle")
        .property("latency", 60u32)
        .build()
        .map_err(|_| anyhow!("missing element: webrtcbin"))?;

    let jb = mk("rtpjitterbuffer", "voice_jb")?;
    jb.set_property("latency", 40u32);
    let depay = mk("rtpopusdepay", "voice_depay")?;
    let dec = mk("opusdec", "voice_dec")?;
    dec.set_property("plc", true);
    let conv = mk("audioconvert", "voice_conv")?;
    let res = mk("audioresample", "voice_res")?;

    // Roh-PCM-Weiche: ein Zweig zu Opus (Pi-Client, Viewer, Standard), ein Zweig zu
    // unkomprimiertem L16 (ESP32 & andere Geräte ohne Opus-Decoder).
    let raw_tee = mk("tee", "voice_raw_tee")?;
    raw_tee.set_property("allow-not-linked", true);

    // --- Opus-Zweig (unverändertes bisheriges Verhalten) ---
    let q_opus_in = mk("queue", "voice_q_opus_in")?;
    let enc = mk("opusenc", "voice_enc")?;
    enc.set_property("bitrate", 48000i32);
    enc.set_property_from_str("frame-size", "20"); // 20 ms Rahmen (Enum-Property)
    let pay = mk("rtpopuspay", "voice_pay")?;
    pay.set_property("pt", OPUS_PT as u32);
    let tee = mk("tee", "voice_tee")?;
    tee.set_property("allow-not-linked", true);

    let q_udp = mk("queue", "voice_q_udp")?;
    q_udp.set_property_from_str("leaky", "downstream");
    let udpsink = mk("multiudpsink", "voice_udp")?;
    udpsink.set_property("sync", false);
    udpsink.set_property("async", false);

    // --- L16-Zweig (neu, für ESP32 & Co.) ---
    let q_l16_in = mk("queue", "voice_q_l16_in")?;
    let l16_conv = mk("audioconvert", "voice_l16_conv")?;
    let l16_caps = mk("capsfilter", "voice_l16_caps")?;
    l16_caps.set_property(
        "caps",
        &gst::Caps::builder("audio/x-raw")
            .field("format", "S16BE")
            .field("rate", L16_RATE)
            .field("channels", 1i32)
            .field("layout", "interleaved")
            .build(),
    );
    let l16_pay = mk("rtpL16pay", "voice_l16_pay")?;
    l16_pay.set_property("pt", L16_PT as u32);
    let q_l16_udp = mk("queue", "voice_q_l16_udp")?;
    q_l16_udp.set_property_from_str("leaky", "downstream");
    let l16_udpsink = mk("multiudpsink", "voice_l16_udp")?;
    l16_udpsink.set_property("sync", false);
    l16_udpsink.set_property("async", false);

    pipeline.add_many([
        &webrtc, &jb, &depay, &dec, &conv, &res, &raw_tee,
        &q_opus_in, &enc, &pay, &tee, &q_udp, &udpsink,
        &q_l16_in, &l16_conv, &l16_caps, &l16_pay, &q_l16_udp, &l16_udpsink,
    ])?;
    gst::Element::link_many([&jb, &depay, &dec, &conv, &res, &raw_tee])?;
    gst::Element::link_many([&q_opus_in, &enc, &pay, &tee])?;
    gst::Element::link_many([&q_udp, &udpsink])?;
    gst::Element::link_many([&q_l16_in, &l16_conv, &l16_caps, &l16_pay, &q_l16_udp, &l16_udpsink])?;

    let tee_src = tee
        .request_pad_simple("src_%u")
        .ok_or_else(|| anyhow!("tee src pad"))?;
    tee_src.link(&q_udp.static_pad("sink").ok_or_else(|| anyhow!("queue sink"))?)?;

    let raw_tee_opus = raw_tee
        .request_pad_simple("src_%u")
        .ok_or_else(|| anyhow!("raw_tee src pad (opus)"))?;
    raw_tee_opus.link(&q_opus_in.static_pad("sink").ok_or_else(|| anyhow!("q_opus_in sink"))?)?;

    let raw_tee_l16 = raw_tee
        .request_pad_simple("src_%u")
        .ok_or_else(|| anyhow!("raw_tee src pad (l16)"))?;
    raw_tee_l16.link(&q_l16_in.static_pad("sink").ok_or_else(|| anyhow!("q_l16_in sink"))?)?;

    // Eingang vom Browser dynamisch verbinden
    let jb_clone = jb.clone();
    webrtc.connect_pad_added(move |_w, pad| {
        if pad.name().starts_with("src_") {
            if let Some(sink) = jb_clone.static_pad("sink") {
                if let Err(e) = pad.link(&sink) {
                    tracing::warn!(?e, "voice: link webrtc→jitterbuffer failed");
                }
            }
        }
    });

    pipeline.set_state(gst::State::Playing)?;

    let handle = VoiceHandle {
        pipeline,
        webrtc,
        tee,
        udpsink,
        l16_udpsink,
        target: Arc::new(std::sync::RwLock::new(target.clone())),
        viewer_branches: Arc::new(Mutex::new(Vec::new())),
    };

    // Alle Online-Lautsprecher des Ziels als UDP-Ziele eintragen (je nach Codec auf
    // den Opus- oder den L16-Ausgang).
    let targets = online_for(target).await;
    for s in &targets {
        udp_add(speaker_sink(&handle, &s.codec), s.ip, s.rtp_port);
    }
    tracing::info!(target = %target.label(), speakers = targets.len(), "🔊 voice session started");

    *VOICE_SESSION.lock().await = Some(handle.clone());
    VOICE_NONCE.store(VOICE_NONCE_NEXT.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
    Ok(handle)
}

fn add_viewer_branch(handle: &VoiceHandle) -> Result<(gst::Element, Box<dyn FnOnce() + Send>, u64)> {
    let id = VIEWER_BRANCH_ID.fetch_add(1, Ordering::Relaxed);
    let pipeline = handle.pipeline.clone();
    let tee = handle.tee.clone();

    let q = gst::ElementFactory::make("queue")
        .name(&format!("voice_viewer_q_{id}"))
        .build()
        .map_err(|_| anyhow!("missing element: queue"))?;
    let capsf = gst::ElementFactory::make("capsfilter")
        .name(&format!("voice_viewer_cf_{id}"))
        .build()
        .map_err(|_| anyhow!("missing element: capsfilter"))?;
    // Opus steht in SDP immer als „opus/48000/2“ (RFC 7587), auch bei Mono. rtpopuspay
    // setzt encoding-params nur bei Stereo-Eingang; die Durchsage ist Mono → im Angebot
    // stand „OPUS/48000“, Chrome erkannte den Codec nicht und verwarf die Audiozeile still
    // (Viewer/Pager bekamen keinen Ton, ICE startete nie).
    capsf.set_property(
        "caps",
        &gst::Caps::builder("application/x-rtp")
            .field("media", "audio")
            .field("encoding-name", "OPUS")
            .field("payload", OPUS_PT)
            .field("clock-rate", 48_000i32)
            .field("encoding-params", "2")
            .build(),
    );
    let webrtc = gst::ElementFactory::make("webrtcbin")
        .name(&format!("voice_viewer_webrtc_{id}"))
        .property_from_str("bundle-policy", "max-bundle")
        .property("latency", 60u32)
        .build()
        .map_err(|_| anyhow!("missing element: webrtcbin"))?;

    pipeline.add_many([&q, &capsf, &webrtc])?;
    gst::Element::link_many([&q, &capsf])?;
    let tee_src = tee.request_pad_simple("src_%u").ok_or_else(|| anyhow!("tee src pad"))?;
    let webrtc_sink = webrtc
        .request_pad_simple("sink_%u")
        .ok_or_else(|| anyhow!("webrtc sink pad"))?;
    capsf
        .static_pad("src")
        .ok_or_else(|| anyhow!("capsfilter src pad"))?
        .link(&webrtc_sink)?;
    for e in [&q, &capsf, &webrtc] {
        e.sync_state_with_parent().map_err(|e| anyhow!("sync state failed: {e:?}"))?;
    }
    // Erst an den laufenden tee hängen, wenn der Zweig läuft – ein gestoppter Zweig
    // lehnt die Sticky-Events mit FLUSHING ab und tee hält dann den ganzen Funk an.
    tee_src.link(&q.static_pad("sink").ok_or_else(|| anyhow!("queue sink pad"))?)?;

    let webrtc_c = webrtc.clone();
    let cleanup = move || {
        let pipeline = pipeline.clone();
        let tee = tee.clone();
        let webrtc = webrtc_c.clone();
        MainContext::default().invoke(move || {
            let _ = tee.release_request_pad(&tee_src);
            let _ = webrtc.release_request_pad(&webrtc_sink);
            for e in [&webrtc, &capsf, &q] {
                let _ = e.set_state(gst::State::Null);
                let _ = pipeline.remove(e);
            }
        });
    };
    Ok((webrtc, Box::new(cleanup), id))
}

async fn broadcast_to_viewers(ctx: &AppCtx, payload: Value) {
    let ids: Vec<i64> = ctx.viewer_channels.lock().await.keys().copied().collect();
    for id in ids {
        crate::routes::send_viewer_command(ctx, id, payload.clone()).await;
    }
}

type WsSink = futures_util::stream::SplitSink<WebSocket, Message>;

async fn send_json(ws_tx: &Arc<Mutex<WsSink>>, msg: Value) {
    let mut tx = ws_tx.lock().await;
    let _ = tx.send(Message::Text(msg.to_string())).await;
}

fn parse_flag(v: Option<&str>) -> bool {
    matches!(v, Some("1") | Some("true") | Some("yes") | Some("on"))
}

// ============================================================================
// WebSocket: Sender (Browser-Mikrofon)  /ws/voice/tx?group=…&viewers=1
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct TxParams {
    pub group: Option<String>,
    pub viewers: Option<String>,
    /// Alt-Name des Parameters (Kompatibilität zu /ws/paging)
    pub forward_viewers: Option<String>,
    /// Login-Token (JWT) – Durchsagen nur für eingeloggte Nutzer
    pub token: Option<String>,
}

pub async fn tx_ws(
    ws: WebSocketUpgrade,
    Query(params): Query<TxParams>,
    State(ctx): State<AppCtx>,
) -> axum::response::Response {
    // In die Raumlautsprecher sprechen nur eingeloggt
    let auth = crate::stream_auth::WsAuthQuery { token: params.token.clone(), ticket: None };
    if crate::stream_auth::user_from_query(&ctx, &auth).is_none() {
        tracing::warn!("voice tx: Verbindung ohne gültiges Login abgelehnt");
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut forward_viewers =
        parse_flag(params.viewers.as_deref()) || parse_flag(params.forward_viewers.as_deref());
    let group = params
        .group
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty())
        .unwrap_or_else(|| "default".to_string());

    ws.on_upgrade(move |socket| async move {
        let (ws_tx, mut ws_rx) = socket.split();
        let ws_tx: Arc<Mutex<WsSink>> = Arc::new(Mutex::new(ws_tx));

        let target = match resolve_target(&ctx, &group).await {
            Ok(t) => t,
            Err(e) => {
                send_json(&ws_tx, json!({"type":"error","message": e.to_string()})).await;
                return;
            }
        };
        let handle = match start_voice_session(&target).await {
            Ok(h) => h,
            Err(e) => {
                send_json(&ws_tx, json!({"type":"error","message": e.to_string()})).await;
                return;
            }
        };
        let webrtc = handle.webrtc.clone();
        let viewer_path = "/ws/voice/viewer";

        notify_target(&target, &json!({"type":"stream","active":true})).await;
        VIEWER_PAGING.store(forward_viewers, Ordering::Relaxed);
        if forward_viewers {
            broadcast_to_viewers(&ctx, json!({"action":"paging_start","ws":viewer_path})).await;
        }

        // Status-Task: alle 2 s Zähler an den Sender
        let status_task = {
            let ws_tx = ws_tx.clone();
            let handle = handle.clone();
            tokio::spawn(async move {
                loop {
                    let target = handle.target();
                    let list = online_for(&target).await;
                    let receiving = list.iter().filter(|s| s.is_receiving()).count();
                    let msg = json!({
                        "type":"status",
                        "group": target.label(),
                        "speakers_online": list.len(),
                        "speakers_receiving": receiving,
                        "speakers": list.iter().map(|s| json!({"name": s.name, "receiving": s.is_receiving()})).collect::<Vec<_>>(),
                    });
                    {
                        let mut tx = ws_tx.lock().await;
                        if tx.send(Message::Text(msg.to_string())).await.is_err() {
                            break;
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            })
        };

        // ICE Server → Browser
        {
            let rt = tokio::runtime::Handle::current();
            let ws_tx = ws_tx.clone();
            webrtc.connect("on-ice-candidate", false, move |values| {
                let mline = values[1].get::<u32>().unwrap_or(0);
                let cand = values[2].get::<String>().unwrap_or_default();
                let ws_tx = ws_tx.clone();
                rt.spawn(async move {
                    let mut tx = ws_tx.lock().await;
                    let _ = tx
                        .send(Message::Text(
                            json!({"type":"ice","candidate":cand,"sdpMLineIndex":mline}).to_string(),
                        ))
                        .await;
                });
                None
            });
        }

        while let Some(Ok(Message::Text(txt))) = ws_rx.next().await {
            let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
            match v.get("type").and_then(|s| s.as_str()) {
                Some("offer") => {
                    let Some(sdp_str) = v.get("sdp").and_then(|s| s.as_str()) else { continue };
                    let Ok(sdp) = gst_sdp::SDPMessage::parse_buffer(sdp_str.as_bytes()) else {
                        send_json(&ws_tx, json!({"type":"error","message":"SDP unlesbar"})).await;
                        continue;
                    };
                    let desc = gst_webrtc::WebRTCSessionDescription::new(gst_webrtc::WebRTCSDPType::Offer, sdp);
                    webrtc.emit_by_name::<()>("set-remote-description", &[&desc, &None::<gst::Promise>]);
                    let ws_tx2 = ws_tx.clone();
                    let webrtc2 = webrtc.clone();
                    let rt = tokio::runtime::Handle::current();
                    let promise = gst::Promise::with_change_func(move |reply| {
                        let answer = match reply {
                            Ok(Some(r)) => r.get::<gst_webrtc::WebRTCSessionDescription>("answer").map_err(|_| format!("{r:?}")),
                            other => Err(format!("{other:?}")),
                        };
                        let answer = match answer {
                            Ok(a) => a,
                            Err(reason) => {
                                tracing::warn!(%reason, "voice tx: create-answer fehlgeschlagen");
                                let ws_tx2 = ws_tx2.clone();
                                rt.spawn(async move {
                                    let mut tx = ws_tx2.lock().await;
                                    let _ = tx.send(Message::Text(json!({"type":"error","message":"Server konnte die Verbindung nicht annehmen"}).to_string())).await;
                                });
                                return;
                            }
                        };
                        webrtc2.emit_by_name::<()>("set-local-description", &[&answer, &None::<gst::Promise>]);
                        if let Ok(sdp_text) = answer.sdp().as_text() {
                            let ws_tx2 = ws_tx2.clone();
                            rt.spawn(async move {
                                let mut tx = ws_tx2.lock().await;
                                let _ = tx
                                    .send(Message::Text(json!({"type":"answer","sdp":sdp_text}).to_string()))
                                    .await;
                            });
                        }
                    });
                    webrtc.emit_by_name::<()>("create-answer", &[&None::<gst::Structure>, &promise]);
                }
                Some("ice") => {
                    if let (Some(c), Some(m)) = (
                        v.get("candidate").and_then(|s| s.as_str()),
                        v.get("sdpMLineIndex").and_then(|n| n.as_u64()),
                    ) {
                        webrtc.emit_by_name::<()>("add-ice-candidate", &[&(m as u32), &c]);
                    }
                }
                Some("config") => {
                    if let Some(fwd) = v.get("viewers").or_else(|| v.get("forward_to_viewers")).and_then(|b| b.as_bool()) {
                        if fwd && !forward_viewers {
                            broadcast_to_viewers(&ctx, json!({"action":"paging_start","ws":viewer_path})).await;
                        } else if !fwd && forward_viewers {
                            broadcast_to_viewers(&ctx, json!({"action":"paging_stop"})).await;
                        }
                        forward_viewers = fwd;
                        VIEWER_PAGING.store(fwd, Ordering::Relaxed);
                    }
                }
                Some("bye") => break,
                _ => {}
            }
        }

        status_task.abort();
        VIEWER_PAGING.store(false, Ordering::Relaxed);
        if forward_viewers {
            broadcast_to_viewers(&ctx, json!({"action":"paging_stop"})).await;
        }
        notify_target(&handle.target(), &json!({"type":"stream","active":false})).await;
        stop_voice().await;
    })
    .into_response()
}

// ============================================================================
// WebSocket: Viewer-Hörer  /ws/voice/viewer  (Audio-only WebRTC aus der laufenden Sitzung)
// ============================================================================

pub async fn viewer_ws(
    ws: WebSocketUpgrade,
    Query(q): Query<crate::stream_auth::WsAuthQuery>,
    State(ctx): State<AppCtx>,
) -> axum::response::Response {
    // Mithören nur eingeloggt oder mit Ticket der laufenden Durchsage
    if !crate::stream_auth::may_hear_paging(&ctx, &q) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |mut socket| async move {
        let Some(handle) = VOICE_SESSION.lock().await.clone() else {
            let _ = socket
                .send(Message::Text(json!({"type":"error","message":"voice not active"}).to_string()))
                .await;
            return;
        };
        let (webrtc, cleanup, branch_id) = match add_viewer_branch(&handle) {
            Ok(v) => v,
            Err(e) => {
                let _ = socket
                    .send(Message::Text(json!({"type":"error","message":e.to_string()}).to_string()))
                    .await;
                return;
            }
        };
        handle.viewer_branches.lock().await.push(VoiceViewerBranch { id: branch_id, cleanup });

        let (ws_tx, mut ws_rx) = socket.split();
        let ws_tx = Arc::new(Mutex::new(ws_tx));
        {
            let rt = tokio::runtime::Handle::current();
            let ws_tx = ws_tx.clone();
            webrtc.connect("on-ice-candidate", false, move |values| {
                let mline = values[1].get::<u32>().unwrap_or(0);
                let cand = values[2].get::<String>().unwrap_or_default();
                let ws_tx = ws_tx.clone();
                rt.spawn(async move {
                    let mut tx = ws_tx.lock().await;
                    let _ = tx
                        .send(Message::Text(json!({"type":"ice","candidate":cand,"sdpMLineIndex":mline}).to_string()))
                        .await;
                });
                None
            });
        }
        {
            let ws_tx = ws_tx.clone();
            let webrtc2 = webrtc.clone();
            let rt = tokio::runtime::Handle::current();
            let promise = gst::Promise::with_change_func(move |reply| {
                let Ok(Some(reply)) = reply else { return };
                let Ok(offer) = reply.get::<gst_webrtc::WebRTCSessionDescription>("offer") else { return };
                webrtc2.emit_by_name::<()>("set-local-description", &[&offer, &None::<gst::Promise>]);
                if let Ok(sdp_text) = offer.sdp().as_text() {
                    let ws_tx = ws_tx.clone();
                    rt.spawn(async move {
                        let mut tx = ws_tx.lock().await;
                        let _ = tx
                            .send(Message::Text(json!({"type":"offer","sdp":sdp_text}).to_string()))
                            .await;
                    });
                }
            });
            webrtc.emit_by_name::<()>("create-offer", &[&None::<gst::Structure>, &promise]);
        }

        while let Some(Ok(Message::Text(txt))) = ws_rx.next().await {
            let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
            match v.get("type").and_then(|s| s.as_str()) {
                Some("answer") => {
                    if let Some(sdp) = v.get("sdp").and_then(|s| s.as_str()) {
                        if let Ok(parsed) = gst_sdp::SDPMessage::parse_buffer(sdp.as_bytes()) {
                            let desc = gst_webrtc::WebRTCSessionDescription::new(gst_webrtc::WebRTCSDPType::Answer, parsed);
                            webrtc.emit_by_name::<()>("set-remote-description", &[&desc, &None::<gst::Promise>]);
                        }
                    }
                }
                Some("ice") => {
                    if let (Some(c), Some(m)) = (
                        v.get("candidate").and_then(|s| s.as_str()),
                        v.get("sdpMLineIndex").and_then(|n| n.as_u64()),
                    ) {
                        webrtc.emit_by_name::<()>("add-ice-candidate", &[&(m as u32), &c]);
                    }
                }
                Some("bye") => break,
                _ => {}
            }
        }

        let mut branches = handle.viewer_branches.lock().await;
        if let Some(pos) = branches.iter().position(|b| b.id == branch_id) {
            let b = branches.remove(pos);
            (b.cleanup)();
        }
    })
    .into_response()
}

// ============================================================================
// WebSocket: Lautsprecher  /ws/voice/speaker
// ============================================================================

#[derive(Debug, Deserialize)]
struct RegisterMsg {
    device_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    group: Option<String>,
    #[serde(default = "default_rtp_port")]
    rtp_port: u16,
    #[serde(default = "default_codec")]
    codec: String,
    #[serde(default)]
    version: Option<String>,
    /// Optional: eigene IP, falls der Server sie nicht aus der Verbindung ableiten kann
    #[serde(default)]
    ip: Option<String>,
    /// Firmware kann einen Geräteschlüssel speichern (neue Firmware)
    #[serde(default)]
    key_support: bool,
    /// Gespeicherter Geräteschlüssel aus der Freigabe
    #[serde(default)]
    key: Option<String>,
}
fn default_rtp_port() -> u16 {
    5004
}
fn default_codec() -> String {
    "opus".into()
}

pub async fn speaker_ws(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(ctx): State<AppCtx>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        let (mut ws_tx, mut ws_rx) = socket.split();

        // 1) Registrierung abwarten (max. 10 s)
        let reg: RegisterMsg = loop {
            let next = tokio::time::timeout(Duration::from_secs(10), ws_rx.next()).await;
            match next {
                Ok(Some(Ok(Message::Text(txt)))) => {
                    let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
                    if v.get("type").and_then(|s| s.as_str()) != Some("register") {
                        continue;
                    }
                    match serde_json::from_value::<RegisterMsg>(v) {
                        Ok(r) if !r.device_id.trim().is_empty() => break r,
                        _ => {
                            let _ = ws_tx
                                .send(Message::Text(json!({"type":"error","message":"register: device_id fehlt"}).to_string()))
                                .await;
                            return;
                        }
                    }
                }
                Ok(Some(Ok(_))) => continue,
                _ => {
                    tracing::debug!(%addr, "voice speaker: no register message, closing");
                    return;
                }
            }
        };

        let device_id: String = reg.device_id.trim().chars().take(64).collect();
        let ip: IpAddr = reg
            .ip
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(addr.ip());
        let ip = match ip {
            // IPv4-mapped IPv6 (::ffff:a.b.c.d) auf IPv4 zurückführen
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
            other => other,
        };

        // 2) DB-Eintrag (Name/Gruppe aus DB gewinnen, falls vorhanden)
        let row = match crate::db::speaker_upsert(
            &ctx.db,
            &device_id,
            reg.name.as_deref().unwrap_or("Lautsprecher"),
            reg.group.as_deref().unwrap_or("default"),
            &ip.to_string(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(?e, "voice speaker: db upsert failed");
                return;
            }
        };

        // 2b) Freigabe prüfen. Wer einmal mit Schlüssel freigegeben wurde, muss ihn immer
        // vorzeigen – sonst könnte sich jedes Gerät mit derselben Geräte-ID als „alt“ ausgeben.
        let stored = crate::pairing::speaker_key_hash(&ctx.db, row.id).await;
        let presented = reg.key.as_deref().map(str::trim).filter(|k| !k.is_empty());
        let auth: &'static str = match (&stored, presented) {
            (Some(h), Some(k)) if *h == crate::pairing::hash_key(k) => "approved",
            (Some(_), _) => "pending",
            (None, _) if reg.key_support => "pending",
            (None, _) => "legacy",
        };
        if auth == "pending" {
            // Ein freigegebenes Gerät mit derselben ID ist online → nicht verdrängen lassen
            let approved_online = SPEAKERS
                .lock()
                .await
                .get(&device_id)
                .is_some_and(|s| s.is_online() && s.auth == "approved");
            if approved_online {
                tracing::warn!(device = %device_id, %ip, "🔈 Anmeldung ohne gültigen Schlüssel abgelehnt (Gerät ist bereits freigegeben online)");
                let _ = ws_tx
                    .send(Message::Text(json!({"type":"error","message":"Gerät ist bereits angemeldet"}).to_string()))
                    .await;
                return;
            }
            if stored.is_some() {
                tracing::warn!(device = %device_id, %ip, "🔈 Lautsprecher mit falschem/fehlendem Schlüssel – wartet auf erneute Freigabe");
            }
        }

        // 3) Live-Eintrag + Writer-Task
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let live = SpeakerLive {
            device_id: device_id.clone(),
            db_id: row.id,
            name: row.name.clone(),
            group: row.group_name.clone(),
            ip,
            rtp_port: reg.rtp_port,
            codec: reg.codec.clone(),
            version: reg.version.clone().unwrap_or_default(),
            volume: row.volume,
            muted: row.muted != 0,
            playing: false,
            rms_db: -120.0,
            last_seen: Instant::now(),
            connected_at: Instant::now(),
            last_heartbeat_playing: None,
            auth,
            tx: tx.clone(),
        };
        {
            let mut map = SPEAKERS.lock().await;
            if let Some(old) = map.insert(device_id.clone(), live.clone()) {
                // alte Verbindung desselben Geräts ersetzen
                let _ = old.tx.send(json!({"type":"replaced"}).to_string());
            }
        }
        let writer = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if ws_tx.send(Message::Text(msg)).await.is_err() {
                    break;
                }
            }
        });
        let _ = tx.send(
            json!({
                "type":"registered",
                "id": row.id,
                "name": row.name,
                "group": row.group_name,
                "volume": row.volume,
                "muted": row.muted != 0,
                "ip": ip.to_string(),
                "rtp_pt": if reg.codec.eq_ignore_ascii_case("l16") { L16_PT } else { OPUS_PT },
                "auth": auth,
            })
            .to_string(),
        );
        tracing::info!(device = %device_id, name = %row.name, group = %row.group_name, codec = %reg.codec, %ip, port = reg.rtp_port, auth, "🔈 speaker registered");

        // Läuft gerade eine Durchsage für diese Gruppe? Dann sofort als Ziel eintragen.
        if let Some(h) = VOICE_SESSION.lock().await.clone() {
            if auth != "pending" && h.target().matches(&row.group_name, row.id) {
                udp_add(speaker_sink(&h, &reg.codec), ip, reg.rtp_port);
                let _ = tx.send(json!({"type":"stream","active":true}).to_string());
            }
        }

        // 4) Heartbeats / Acks
        let mut touch_counter = 0u32;
        while let Some(Ok(msg)) = ws_rx.next().await {
            let Message::Text(txt) = msg else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
            match v.get("type").and_then(|s| s.as_str()) {
                Some("heartbeat") => {
                    let playing = v.get("playing").and_then(|b| b.as_bool()).unwrap_or(false);
                    let rms = v.get("rms_db").and_then(|n| n.as_f64()).unwrap_or(-120.0);
                    let mut map = SPEAKERS.lock().await;
                    if let Some(s) = map.get_mut(&device_id) {
                        s.last_seen = Instant::now();
                        s.playing = playing;
                        s.rms_db = rms;
                        if playing {
                            s.last_heartbeat_playing = Some(Instant::now());
                        }
                        if let Some(m) = v.get("muted").and_then(|b| b.as_bool()) {
                            s.muted = m;
                        }
                        if let Some(vol) = v.get("volume").and_then(|n| n.as_i64()) {
                            s.volume = vol;
                        }
                    }
                    drop(map);
                    touch_counter += 1;
                    if touch_counter % 12 == 0 {
                        let _ = crate::db::speaker_touch(&ctx.db, row.id, &ip.to_string()).await;
                    }
                }
                Some("ack") => {
                    let cmd = v.get("cmd").and_then(|s| s.as_str()).unwrap_or("?");
                    let ok = v.get("ok").and_then(|b| b.as_bool()).unwrap_or(true);
                    if ok {
                        tracing::debug!(device = %device_id, cmd, "speaker ack");
                    } else {
                        tracing::warn!(device = %device_id, cmd, error = ?v.get("error"), "speaker command failed");
                    }
                }
                Some("bye") => break,
                _ => {}
            }
        }

        // 5) Abmelden
        writer.abort();
        let removed = {
            let mut map = SPEAKERS.lock().await;
            match map.get(&device_id) {
                // Nur entfernen, wenn nicht inzwischen eine neue Verbindung desselben Geräts steht
                Some(cur) if cur.connected_at == live.connected_at => map.remove(&device_id),
                _ => None,
            }
        };
        if removed.is_some() {
            if let Some(h) = VOICE_SESSION.lock().await.clone() {
                udp_remove(speaker_sink(&h, &reg.codec), ip, reg.rtp_port);
            }
            let _ = crate::db::speaker_touch(&ctx.db, row.id, &ip.to_string()).await;
            tracing::info!(device = %device_id, "🔈 speaker disconnected");
        }
    })
}

// ============================================================================
// REST: /api/voice/*
// ============================================================================

#[derive(Serialize)]
pub struct SpeakerDto {
    pub id: i64,
    pub device_id: String,
    pub name: String,
    pub group: String,
    pub volume: i64,
    pub muted: bool,
    pub online: bool,
    pub receiving: bool,
    pub playing: bool,
    pub rms_db: Option<f64>,
    pub ip: Option<String>,
    pub rtp_port: Option<u16>,
    pub codec: Option<String>,
    pub version: Option<String>,
    pub last_seen: Option<i64>,
    pub created_at: i64,
    /// "approved" | "pending" | "legacy" (online) bzw. "approved" | "unknown" (offline)
    pub auth: String,
}

pub async fn speakers_list(State(ctx): State<AppCtx>) -> Result<Json<Value>, StatusCode> {
    let rows = crate::db::speakers_list(&ctx.db).await.map_err(|e| {
        tracing::error!(?e, "speakers_list");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let keyed: std::collections::HashSet<i64> = sqlx::query_scalar::<_, i64>("SELECT speaker_id FROM speaker_keys")
        .fetch_all(&ctx.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    let live = SPEAKERS.lock().await;
    let list: Vec<SpeakerDto> = rows
        .iter()
        .map(|r| {
            let l = live.get(&r.device_id).filter(|l| l.is_online());
            let auth = match l {
                Some(l) => l.auth.to_string(),
                None if keyed.contains(&r.id) => "approved".into(),
                None => "unknown".into(),
            };
            SpeakerDto {
                id: r.id,
                device_id: r.device_id.clone(),
                name: r.name.clone(),
                group: r.group_name.clone(),
                volume: r.volume,
                muted: l.map(|l| l.muted).unwrap_or(r.muted != 0),
                online: l.is_some(),
                receiving: l.map(|l| l.is_receiving()).unwrap_or(false),
                playing: l.map(|l| l.playing).unwrap_or(false),
                rms_db: l.map(|l| l.rms_db),
                ip: l.map(|l| l.ip.to_string()).or_else(|| r.last_ip.clone()),
                rtp_port: l.map(|l| l.rtp_port),
                codec: l.map(|l| l.codec.clone()),
                version: l.map(|l| l.version.clone()),
                last_seen: r.last_seen,
                created_at: r.created_at,
                auth,
            }
        })
        .collect();
    let active = VOICE_SESSION.lock().await.as_ref().map(|h| h.target().label());
    Ok(Json(json!({ "speakers": list, "active_group": active })))
}

pub async fn groups_list(State(ctx): State<AppCtx>) -> Result<Json<Value>, StatusCode> {
    let rows = crate::db::speakers_list(&ctx.db).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let live = SPEAKERS.lock().await;
    let mut groups: HashMap<String, (usize, usize)> = HashMap::new();
    for r in &rows {
        let e = groups.entry(r.group_name.clone()).or_insert((0, 0));
        e.0 += 1;
        if live.get(&r.device_id).map(|l| l.is_online()).unwrap_or(false) {
            e.1 += 1;
        }
    }
    let mut out: Vec<Value> = groups
        .into_iter()
        .map(|(g, (total, online))| json!({"group": g, "total": total, "online": online}))
        .collect();
    out.sort_by(|a, b| a["group"].as_str().cmp(&b["group"].as_str()));
    Ok(Json(json!({ "groups": out })))
}

#[derive(Deserialize)]
pub struct SpeakerPatch {
    pub name: Option<String>,
    pub group: Option<String>,
    pub volume: Option<i64>,
    pub muted: Option<bool>,
}

pub async fn speaker_patch(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
    Json(body): Json<SpeakerPatch>,
) -> Result<Json<Value>, StatusCode> {
    let old = crate::db::speaker_get(&ctx.db, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let name = body
        .name
        .map(|s| s.trim().chars().take(80).collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or(old.name.clone());
    let group = body
        .group
        .map(|s| s.trim().chars().take(40).collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or(old.group_name.clone());
    let volume = body.volume.unwrap_or(old.volume).clamp(0, 100);
    let muted = body.muted.unwrap_or(old.muted != 0);
    crate::db::speaker_update(&ctx.db, id, &name, &group, volume, muted)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Live-Eintrag angleichen + Gerät informieren
    let mut map = SPEAKERS.lock().await;
    if let Some(s) = map.get_mut(&old.device_id) {
        let group_changed = !s.group.eq_ignore_ascii_case(&group);
        let old_group = s.group.clone();
        s.name = name.clone();
        s.group = group.clone();
        s.volume = volume;
        s.muted = muted;
        s.send(&json!({"type":"set_volume","volume":volume}));
        s.send(&json!({"type":"mute","on":muted}));
        s.send(&json!({"type":"config","name":name,"group":group}));
        if group_changed && s.may_receive() {
            let (ip, port, codec) = (s.ip, s.rtp_port, s.codec.clone());
            drop(map);
            if let Some(h) = VOICE_SESSION.lock().await.clone() {
                let t = h.target();
                let (was, is) = (t.matches(&old_group, id), t.matches(&group, id));
                if was && !is {
                    udp_remove(speaker_sink(&h, &codec), ip, port);
                }
                if is && !was {
                    udp_add(speaker_sink(&h, &codec), ip, port);
                }
            }
        }
    }
    Ok(Json(json!({"ok": true, "id": id, "name": name, "group": group, "volume": volume, "muted": muted})))
}

#[derive(Deserialize)]
pub struct SpeakerCmd {
    pub cmd: String,
    #[serde(default)]
    pub value: Option<Value>,
}

/// Lautsprecher freigeben: Schlüssel erzeugen, dem Gerät schicken (es speichert ihn
/// dauerhaft) und sofort als freigegeben behandeln. Läuft gerade eine Durchsage an sein Ziel,
/// bekommt es direkt Ton.
pub async fn speaker_approve(
    user: crate::routes::AuthUser,
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    let row = crate::db::speaker_get(&ctx.db, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let online = SPEAKERS.lock().await.get(&row.device_id).is_some_and(|s| s.is_online());
    if !online {
        // Den Schlüssel kann nur ein verbundenes Gerät entgegennehmen
        return Ok(Json(json!({"ok": false, "error": "Lautsprecher ist offline"})));
    }
    let key = crate::pairing::speaker_key_create(&ctx.db, id, &user.username)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut map = SPEAKERS.lock().await;
    let Some(s) = map.get_mut(&row.device_id) else { return Ok(Json(json!({"ok": false, "error": "Lautsprecher ist offline"}))) };
    let was = s.auth;
    s.auth = "approved";
    s.send(&json!({"type":"pair","key": key}));
    let (ip, port, codec, group) = (s.ip, s.rtp_port, s.codec.clone(), s.group.clone());
    drop(map);
    if was == "pending" {
        if let Some(h) = VOICE_SESSION.lock().await.clone() {
            if h.target().matches(&group, id) {
                udp_add(speaker_sink(&h, &codec), ip, port);
                notify_device(&row.device_id, &json!({"type":"stream","active":true})).await;
            }
        }
    }
    tracing::info!(speaker = id, by = %user.username, "🔈 Lautsprecher freigegeben");
    Ok(Json(json!({"ok": true})))
}

/// Freigabe entziehen: Schlüssel löschen, Gerät verliert sofort den Ton.
pub async fn speaker_revoke(
    user: crate::routes::AuthUser,
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    let row = crate::db::speaker_get(&ctx.db, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    crate::pairing::speaker_key_delete(&ctx.db, id).await;
    let mut map = SPEAKERS.lock().await;
    if let Some(s) = map.get_mut(&row.device_id) {
        s.auth = "pending";
        s.send(&json!({"type":"unpair"}));
        s.send(&json!({"type":"stream","active":false}));
        let (ip, port, codec) = (s.ip, s.rtp_port, s.codec.clone());
        drop(map);
        if let Some(h) = VOICE_SESSION.lock().await.clone() {
            udp_remove(speaker_sink(&h, &codec), ip, port);
        }
    }
    tracing::info!(speaker = id, by = %user.username, "🔈 Freigabe entzogen");
    Ok(Json(json!({"ok": true})))
}

pub async fn speaker_cmd(
    State(ctx): State<AppCtx>,
    Path(id): Path<i64>,
    Json(body): Json<SpeakerCmd>,
) -> Result<Json<Value>, StatusCode> {
    let row = crate::db::speaker_get(&ctx.db, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let msg = match body.cmd.as_str() {
        "test_tone" => json!({"type":"test_tone","seconds": body.value.as_ref().and_then(|v| v.as_f64()).unwrap_or(1.5)}),
        "identify" => json!({"type":"identify","seconds": body.value.as_ref().and_then(|v| v.as_f64()).unwrap_or(5.0)}),
        "mute" => json!({"type":"mute","on": body.value.as_ref().and_then(|v| v.as_bool()).unwrap_or(true)}),
        "set_volume" => json!({"type":"set_volume","volume": body.value.as_ref().and_then(|v| v.as_i64()).unwrap_or(row.volume).clamp(0,100)}),
        "reboot" => json!({"type":"reboot"}),
        "ping" => json!({"type":"ping"}),
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let delivered = notify_device(&row.device_id, &msg).await;
    if !delivered {
        return Ok(Json(json!({"ok": false, "error": "Lautsprecher ist offline"})));
    }
    Ok(Json(json!({"ok": true})))
}

pub async fn speaker_delete(State(ctx): State<AppCtx>, Path(id): Path<i64>) -> Result<StatusCode, StatusCode> {
    let row = crate::db::speaker_get(&ctx.db, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    crate::db::speaker_delete(&ctx.db, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(s) = SPEAKERS.lock().await.remove(&row.device_id) {
        s.send(&json!({"type":"deleted"}));
        if let Some(h) = VOICE_SESSION.lock().await.clone() {
            udp_remove(speaker_sink(&h, &s.codec), s.ip, s.rtp_port);
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Kurzstatus für UI/Health: läuft eine Durchsage, wie viele Lautsprecher online.
pub async fn voice_status(State(_ctx): State<AppCtx>) -> Json<Value> {
    let active = VOICE_SESSION.lock().await.as_ref().map(|h| h.target().label());
    let live = live_speakers().await;
    let online = live.iter().filter(|s| s.is_online()).count();
    Json(json!({"active": active.is_some(), "active_group": active, "speakers_online": online}))
}
