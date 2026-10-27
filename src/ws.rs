use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::state::AppCtx;
use anyhow;

use glib::ToValue;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_sdp as gst_sdp;
use gstreamer_webrtc as gst_webrtc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WsOut {
    Ready,
    Offer {
        sdp: String,
    },
    Ice {
        candidate: String,
        sdp_mline_index: u32,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WsIn {
    Answer {
        sdp: String,
    },
    Ice {
        candidate: String,
        sdp_mline_index: u32,
    },
}

// -------------------------------------------------------------------------------------
// alter WS (Status / Latenz) + UI-Broadcast
// -------------------------------------------------------------------------------------
pub async fn ws_route(ws: WebSocketUpgrade, State(ctx): State<AppCtx>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = handle_socket(socket, ctx).await {
            eprintln!("[ws] socket error: {e}");
        }
    })
}

async fn handle_socket(socket: WebSocket, ctx: AppCtx) -> Result<(), String> {
    let (ws_tx_raw, mut ws_rx) = socket.split();
    let ws_tx = std::sync::Arc::new(tokio::sync::Mutex::new(ws_tx_raw));

    // als UI-Broadcast-Client registrieren
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    {
        let mut vec = ctx.ui_broadcast.lock().await;
        vec.push(tx);
    }

    // Task: broadcast -> socket
    let tx_clone = ws_tx.clone();
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if tx_clone
                .lock()
                .await
                .send(Message::Text(msg.clone()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // direkt Latenz schicken
    let (last_ns, ewma_ns) = {
        let pm = ctx.pipeline.lock().await;
        pm.latency_snapshot().unwrap_or((0, 0))
    };

    let hello = json!({
        "type": "latency",
        "last_ms": (last_ns as f64) / 1_000_000.0,
        "ewma_ms": (ewma_ns as f64) / 1_000_000.0,
    });
    let _ = ws_tx
        .lock()
        .await
        .send(Message::Text(hello.to_string()))
        .await;

    while let Some(msg_res) = ws_rx.next().await {
        let msg = match msg_res {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[ws] read error: {e}");
                break;
            }
        };

        match msg {
            Message::Text(txt) => match txt.as_str() {
                "latency" => {
                    let (last_ns, ewma_ns) = {
                        let pm = ctx.pipeline.lock().await;
                        pm.latency_snapshot().unwrap_or((0, 0))
                    };
                    let payload = json!({
                        "type": "latency",
                        "last_ms": (last_ns as f64) / 1_000_000.0,
                        "ewma_ms": (ewma_ns as f64) / 1_000_000.0,
                    });
                    let _ = ws_tx
                        .lock()
                        .await
                        .send(Message::Text(payload.to_string()))
                        .await;
                }
                "ping" => {
                    let _ = ws_tx
                        .lock()
                        .await
                        .send(Message::Text("{\"type\":\"pong\"}".into()))
                        .await;
                }
                other => {
                    let _ = ws_tx
                        .lock()
                        .await
                        .send(Message::Text(
                            json!({"type":"echo","msg":other}).to_string(),
                        ))
                        .await;
                }
            },
            Message::Close(_) => break,
            _ => {}
        }
    }

    Ok(())
}

// -------------------------------------------------------------------------------------
// NEUER WebRTC-WS
// -------------------------------------------------------------------------------------
pub async fn handle_webrtc_ws(
    socket: WebSocket,
    pm: Arc<tokio::sync::Mutex<crate::pipeline::PipelineManager>>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) {
    static BRANCH_ID: AtomicU64 = AtomicU64::new(1);
    tracing::info!("ws/webrtc: client connected");

    // ICE-Optionen: Standard jetzt mit TCP-Fallback, IPv4-only; per ENV konfigurierbar
    let ice_tcp_enabled = std::env::var("ORS_ICE_TCP")
        .map(|v| {
            let v = v.to_lowercase();
            !matches!(v.as_str(), "0" | "false" | "no" | "off")
        })
        .unwrap_or(true); // Default: TCP erlaubt
    let ice_ipv6_enabled = matches!(
        std::env::var("ORS_ICE_IPV6")
            .unwrap_or_default()
            .to_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    );
    // STUN: Default wieder Google-STUN, abschaltbar mit ORS_STUN_SERVER=none/off
    let stun_env = std::env::var("ORS_STUN_SERVER").unwrap_or_default();
    let stun_server = match stun_env.to_lowercase().as_str() {
        "" => "stun://stun.l.google.com:19302".to_string(),
        "0" | "false" | "no" | "off" | "none" => String::new(),
        _ => stun_env,
    };

    // Mini-Parser für ICE-Candidate-Strings
    let cand_is_tcp = |cand: &str| {
        cand.split_whitespace()
            .nth(2)
            .map(|p| p.eq_ignore_ascii_case("tcp"))
            .unwrap_or(false)
    };
    let cand_is_ipv6 = |cand: &str| {
        cand.split_whitespace()
            .nth(4)
            .map(|addr| addr.contains(':'))
            .unwrap_or(false)
    };

    // 1) Kanal: GST → WS
    let (sig_tx, mut sig_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();

    // 2) WS splitten
    let (ws_tx, mut ws_rx) = socket.split();
    let ws_tx = std::sync::Arc::new(tokio::sync::Mutex::new(ws_tx));

    // 3) eigenen webrtc-Branch an den Tee hängen (GLib-Thread)
    let (webrtc_tx, webrtc_rx) =
        tokio::sync::oneshot::channel::<Result<(gst::Element, crate::pipeline::SharedCleanup), String>>();
    {
        let pm = pm.clone();
        let branch_id = BRANCH_ID.fetch_add(1, Ordering::Relaxed);
        glib::MainContext::default().invoke(move || {
            let mut mgr = pm.blocking_lock();

            let Some(p) = &mgr.pipeline else {
                tracing::warn!("ws/webrtc: pipeline not running");
                let _ = webrtc_tx.send(Err("pipeline not running".into()));
                return;
            };

            let res = build_webrtc_branch(p, branch_id, mgr.viewer_audio_level())
                .map_err(|e| format!("{e:?}"))
                .map(|(webrtc, cleanup)| {
                    let shared = Arc::new(Mutex::new(Some(cleanup)));
                    mgr.register_cleanup(shared.clone());
                    (webrtc, shared)
                });
            if res.is_ok() {
                tracing::info!("ws/webrtc: branch built (id={})", branch_id);
            } else {
                tracing::warn!("ws/webrtc: branch build failed (id={})", branch_id);
            }
            let _ = webrtc_tx.send(res);
        });
    }

    // 4) auf webrtcbin warten
    let (webrtc, cleanup) = match webrtc_rx.await {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => {
            let _ = ws_tx
                .lock()
                .await
                .send(Message::Text(
                    serde_json::to_string(&WsOut::Error { message: e }).unwrap(),
                ))
                .await;
            return;
        }
        Err(e) => {
            let _ = ws_tx
                .lock()
                .await
                .send(Message::Text(
                    serde_json::to_string(&WsOut::Error {
                        message: e.to_string(),
                    })
                    .unwrap(),
                ))
                .await;
            return;
        }
    };

    // 5) Gemeinsame Zustände für robustes ICE-Handling
    let pending_candidates =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u32, String)>::new()));
    let have_remote_desc = std::sync::Arc::new(std::sync::Mutex::new(false));

    // 6) STUN/TURN & State-Logs & on-ice-candidate (alles im GLib-Thread)
    {
        use glib::prelude::*; // find_property, list_properties, set_property

        let webrtc_props = webrtc.clone();
        let webrtc_states2 = webrtc.clone();
        let webrtc_states3 = webrtc.clone();
        let webrtc_ice = webrtc.clone();
        let sig_tx_ice = sig_tx.clone();

        glib::MainContext::default().invoke(move || {
            // 1) STUN/TURN: Default = Google-STUN, abschaltbar über ORS_STUN_SERVER=off/none
            if !stun_server.is_empty() {
                webrtc_props.set_property_from_str("stun-server", &stun_server);
                tracing::info!("webrtcbin: STUN aktiviert → {}", stun_server);
            } else {
                webrtc_props.set_property_from_str("stun-server", "");
                tracing::info!("webrtcbin: STUN deaktiviert");
            }

            // 2) ICE am Agent: Einstellungen über ENV steuerbar
            if webrtc_props.find_property("ice-agent").is_some() {
                let agent: glib::Object = webrtc_props.property::<glib::Object>("ice-agent");

                // ICE TCP: per ENV
                if agent
                    .list_properties()
                    .iter()
                    .any(|p| p.name() == "ice-tcp")
                {
                    let _ = agent.set_property("ice-tcp", &ice_tcp_enabled);
                    tracing::info!(
                        "webrtcbin: ICE TCP {}",
                        if ice_tcp_enabled {
                            "aktiviert (Default)"
                        } else {
                            "deaktiviert (per ORS_ICE_TCP)"
                        }
                    );
                }

                // IPv6 per ENV (default aus)
                if agent
                    .list_properties()
                    .iter()
                    .any(|p| p.name() == "ice-ipv6")
                {
                    let _ = agent.set_property("ice-ipv6", &ice_ipv6_enabled);
                    tracing::info!(
                        "webrtcbin: ICE IPv6 {}",
                        if ice_ipv6_enabled {
                            "aktiviert (per ORS_ICE_IPV6)"
                        } else {
                            "deaktiviert (Default)"
                        }
                    );
                }

                // Optional (wenn v6 Ärger macht): IPv6 aus
                if agent
                    .list_properties()
                    .iter()
                    .any(|p| p.name() == "controlling-mode")
                {
                    // nix zu tun – nur Beispiel, libnice hat keine simple "ipv6=false" Property.
                    // Wenn v6 Probleme macht: System-/Netzwerkseitig v6 abschalten oder
                    // Browser mit v4-Host testen.
                }
            } else {
                tracing::warn!("webrtcbin: Property 'ice-agent' fehlt");
            }

            // 3) State-Logs (lass sie drin, aber mit echten Zuständen)
            let _ = webrtc_states2.connect("notify::ice-connection-state", false, move |vals| {
                let _ = std::panic::catch_unwind(|| {
                    if let Ok(state) = vals[0].get::<gst::Object>() {
                        let s: gst_webrtc::WebRTCICEConnectionState =
                            state.property("ice-connection-state");
                        tracing::info!("ICE connection state = {:?}", s);
                    }
                });
                None
            });
            let _ = webrtc_states3.connect("notify::connection-state", false, move |vals| {
                let _ = std::panic::catch_unwind(|| {
                    if let Ok(state) = vals[0].get::<gst::Object>() {
                        let s: gst_webrtc::WebRTCPeerConnectionState =
                            state.property("connection-state");
                        tracing::info!("Peer connection state = {:?}", s); // connected/disconnected/failed
                    }
                });
                None
            });

            // --- ICE Server → Browser
            let _ = webrtc_ice.connect("on-ice-candidate", false, move |values| {
                let _ = std::panic::catch_unwind(|| {
                    let mline = values[1].get::<u32>().unwrap_or(0);
                    let cand = values[2].get::<String>().unwrap_or_default();
                    tracing::info!("on-ice-candidate: mline={} cand={}", mline, cand);

                    if !ice_tcp_enabled && cand_is_tcp(&cand) {
                        tracing::info!("on-ice-candidate: TCP unterdrückt");
                        return None::<glib::Value>;
                    }
                    if !ice_ipv6_enabled && cand_is_ipv6(&cand) {
                        tracing::info!("on-ice-candidate: IPv6 unterdrückt");
                        return None::<glib::Value>;
                    }

                    let json = serde_json::to_string(&WsOut::Ice {
                        candidate: cand,
                        sdp_mline_index: mline,
                    })
                    .unwrap();

                    let _ = sig_tx_ice.send(Message::Text(json));
                    None::<glib::Value>
                });
                None
            });
        });
    }

    // 7) Offer erzeugen und an den Client senden
    {
        let webrtc_invoke = webrtc.clone();
        let sig_tx_offer = sig_tx.clone();

        glib::MainContext::default().invoke(move || {
            // Kleines Delay, damit Audio/Transceiver Gelegenheit haben, sich anzumelden
            glib::timeout_add_local_once(Duration::from_millis(200), move || {
                // Zwei getrennte Clones: einer für die Promise, einer für den Call
                let webrtc_for_promise = webrtc_invoke.clone();
                let webrtc_for_call = webrtc_invoke.clone();

                // Promise zum Abholen des Offers
                let sig_tx_offer_cl = sig_tx_offer.clone();
                let promise = gst::Promise::with_change_func(move |reply| {
                    tracing::info!("ws/webrtc: create-offer reply = {:?}", reply);

                    if let Ok(Some(structure)) = reply {
                        if let Ok(offer) =
                            structure.get::<gst_webrtc::WebRTCSessionDescription>("offer")
                        {
                            // 7a) Lokal setzen
                            let none_p: Option<gst::Promise> = None;
                            let args: [&dyn glib::ToValue; 2] =
                                [&offer.to_value(), &none_p.to_value()];
                            webrtc_for_promise.emit_by_name::<()>("set-local-description", &args);

                            // 7b) SDP an Browser
                            if let Ok(sdp_txt) = offer.sdp().as_text() {
                                tracing::info!(
                                    "=== SDP WE SEND TO CLIENT ===\n{}\n==============================",
                                    sdp_txt
                                );
                                let txt =
                                    serde_json::to_string(&WsOut::Offer { sdp: sdp_txt }).unwrap();
                                let _ = sig_tx_offer_cl.send(Message::Text(txt));
                            }
                        }
                    }
                });

                // 7c) Offer anstoßen
                let none_s: Option<gst::Structure> = None;
                let some_p: Option<gst::Promise> = Some(promise);
                let args: [&dyn glib::ToValue; 2] = [&none_s.to_value(), &some_p.to_value()];
                tracing::info!("ws/webrtc: forcing create-offer (delayed 200ms) …");
                webrtc_for_call.emit_by_name::<()>("create-offer", &args);
            });
        });
    }

    // 8) Dem Client signalisieren, dass wir bereit sind (Transceiver/PC darf angelegt werden)
    let _ = ws_tx
        .lock()
        .await
        .send(Message::Text(serde_json::to_string(&WsOut::Ready).unwrap()))
        .await;

    // 9) Task A: GST → WS pumpen
    let ws_tx_pump = ws_tx.clone();
    let pump = tokio::spawn(async move {
        while let Some(msg) = sig_rx.recv().await {
            let _ = ws_tx_pump.lock().await.send(msg).await;
        }
    });

    // 10) Task B: WS → GST (Answer + ICE vom Browser)
    let webrtc_for_set = webrtc.clone();
    loop {
        tokio::select! {
            _ = shutdown.recv() => {
                tracing::info!("ws/webrtc: shutdown signal received");
                let _ = ws_tx.lock().await.send(Message::Close(None)).await;
                break;
            }
            msg = ws_rx.next() => {
                let Some(Ok(msg)) = msg else { break; };
                if let Message::Text(txt) = msg {
                    match serde_json::from_str::<WsIn>(&txt) {
                        Ok(WsIn::Answer { sdp }) => {
                            tracing::info!("ws/webrtc: received Answer (len={})", sdp.len());
                            let sdp_txt = sdp.clone();
                            let webrtc_set = webrtc_for_set.clone();
                            let prc = pending_candidates.clone();
                            let hrd = have_remote_desc.clone();

                            glib::MainContext::default().invoke(move || {
                                match gst_sdp::SDPMessage::parse_buffer(sdp_txt.as_bytes()) {
                                    Ok(sdp_msg) => {
                                        let desc = gst_webrtc::WebRTCSessionDescription::new(
                                            gst_webrtc::WebRTCSDPType::Answer,
                                            sdp_msg,
                                        );

                                        let none_p: Option<gst::Promise> = None;
                                        let args: [&dyn glib::ToValue; 2] =
                                            [&desc.to_value(), &none_p.to_value()];
                                        webrtc_set.emit_by_name::<()>("set-remote-description", &args);
                                        tracing::info!("set-remote-description done");

                                        // Markieren & gepufferte ICE-Kandidaten nachreichen
                                        if let Ok(mut flag) = hrd.lock() {
                                            *flag = true;
                                        }
                                        if let Ok(mut vec) = prc.lock() {
                                            for (m, c) in vec.drain(..) {
                                                let args: [&dyn glib::ToValue; 2] =
                                                    [&m.to_value(), &c.to_value()];
                                                webrtc_set.emit_by_name::<()>("add-ice-candidate", &args);
                                                tracing::info!("flushed queued ICE mline={}", m);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("ws/webrtc: failed to parse Answer SDP: {e:?}");
                                    }
                                }
                            });
                        }

                        Ok(WsIn::Ice {
                            candidate,
                            sdp_mline_index,
                        }) => {
                            tracing::info!(
                                "ws/webrtc: received remote ICE (mline={})",
                                sdp_mline_index
                            );
                            let webrtc_set = webrtc_for_set.clone();
                            let prc = pending_candidates.clone();
                            let hrd = have_remote_desc.clone();
                            let allow_tcp = ice_tcp_enabled;
                            let allow_ipv6 = ice_ipv6_enabled;

                            glib::MainContext::default().invoke(move || {
                                if !allow_tcp && cand_is_tcp(&candidate) {
                                    tracing::info!("remote ICE (TCP) verworfen");
                                    return;
                                }
                                if !allow_ipv6 && cand_is_ipv6(&candidate) {
                                    tracing::info!("remote ICE (IPv6) verworfen");
                                    return;
                                }

                                let have = hrd.lock().map(|f| *f).unwrap_or(false);
                                if have {
                                    let args: [&dyn glib::ToValue; 2] =
                                        [&sdp_mline_index.to_value(), &candidate.to_value()];
                                    webrtc_set.emit_by_name::<()>("add-ice-candidate", &args);
                                    tracing::info!(
                                        "add-ice-candidate (immediate) mline={}",
                                        sdp_mline_index
                                    );
                                } else {
                                    if let Ok(mut vec) = prc.lock() {
                                        vec.push((sdp_mline_index, candidate));
                                        tracing::info!("queued remote ICE (no remote desc yet)");
                                    }
                                }
                            });
                        }

                        Err(e) => {
                            let _ = sig_tx.send(Message::Text(
                                serde_json::to_string(&WsOut::Error {
                                    message: format!("bad json: {e}"),
                                })
                                .unwrap(),
                            ));
                        }
                    }
                }
            }
        }
    }

    // 11) Tasks beenden + Branch aufräumen
    // Nicht auf das Ende der Pump-Task warten: deren Kanal bleibt offen, solange der
    // on-ice-candidate-Handler am webrtcbin einen Sender hält – der Handler käme sonst nie
    // hier an, und jeder Zuschauer hinterließe seinen kompletten Zweig (Speicherleck,
    // weiterlaufender VP8-Encoder an der Pipeline).
    drop(sig_tx);
    pump.abort();
    let to_run = cleanup.lock().unwrap().take();
    if let Some(run) = to_run {
        run();
    }
    tracing::info!("ws/webrtc: client disconnected");
}

/// Ein neu angehängter Zuschauer-Zweig löst ein RECONFIGURE-Ereignis stromaufwärts aus.
/// Der laufende Encoder handelt daraufhin seine Puffer neu aus und fragt dazu alle
/// Abzweige an; diese (serialisierte) Anfrage wartet in jeder Queue, bis sie leer ist –
/// mit dem Aufnahme-Muxer, der Bild und Ton verschränkt, verklemmt sich dabei der ganze
/// Video-Abzweig (Aufnahme, Verteiler und alle Zuschauer frieren ein). Für einen weiteren
/// Abnehmer muss am laufenden Encoder nichts neu ausgehandelt werden, also verwerfen.
pub(crate) fn drop_upstream_reconfigure(tee_src: &gst::Pad) {
    tee_src.add_probe(gst::PadProbeType::EVENT_UPSTREAM, |_, info| {
        if let Some(gst::PadProbeData::Event(ev)) = &info.data {
            if ev.type_() == gst::EventType::Reconfigure {
                return gst::PadProbeReturn::Drop;
            }
        }
        gst::PadProbeReturn::Ok
    });
}

fn build_webrtc_branch(
    pipeline: &gst::Pipeline,
    id: u64,
    (gain, muted): (f64, bool),
) -> anyhow::Result<(gst::Element, crate::pipeline::BranchCleanup)> {
    // Elemente bauen (eindeutige Namen)
    let tee = pipeline
        .by_name("tee_enc")
        .ok_or_else(|| anyhow::anyhow!("tee_enc not found"))?;

    let q_webrtc = gst::ElementFactory::make("queue")
        .name(&format!("q_webrtc_{id}"))
        .build()?;
    let parse_in = gst::ElementFactory::make("h264parse")
        .name(&format!("parse_webrtc_{id}"))
        .property("config-interval", 1i32)
        .build()?;
    let dec = gst::ElementFactory::make("avdec_h264")
        .name(&format!("webrtc_h264dec_{id}"))
        .build()?;
    let vconv = gst::ElementFactory::make("videoconvert")
        .name(&format!("webrtc_vconv_{id}"))
        .build()?;
    let vcf = gst::ElementFactory::make("capsfilter")
        .name(&format!("webrtc_vcaps_{id}"))
        .build()?;
    let v_caps = gst::Caps::builder("video/x-raw")
        .field("format", &"I420")
        .field("framerate", &gst::Fraction::new(30, 1))
        .build();
    vcf.set_property("caps", &v_caps);

    let vp8 = gst::ElementFactory::make("vp8enc")
        .name(&format!("webrtc_vp8enc_{id}"))
        .property_from_str("deadline", "1")
        .property("cpu-used", 8i32)
        .property("target-bitrate", 2_000_000i32)
        .property("keyframe-max-dist", 30i32)
        .build()?;
    let pay = gst::ElementFactory::make("rtpvp8pay")
        .name(&format!("webrtc_rtpvp8pay_{id}"))
        .build()?;
    pay.set_property("pt", 96u32);

    let cf_rtp = gst::ElementFactory::make("capsfilter")
        .name(&format!("webrtc_v_rtp_caps_{id}"))
        .build()?;
    let rtp_caps = gst::Caps::builder("application/x-rtp")
        .field("media", "video")
        .field("encoding-name", "VP8")
        .field("payload", 96i32)
        .field("clock-rate", 90000i32)
        .build();
    cf_rtp.set_property("caps", &rtp_caps);

    let webrtc = gst::ElementFactory::make("webrtcbin")
        .name(&format!("webrtcbin_{id}"))
        .build()
        .map_err(|_| anyhow::anyhow!("missing element: webrtcbin"))?;

    // Alles hinzufügen
    pipeline.add_many(&[
        &q_webrtc, &parse_in, &dec, &vconv, &vcf, &vp8, &pay, &cf_rtp, &webrtc,
    ])?;

    // tee → q
    let tee_src = tee
        .request_pad_simple("src_%u")
        .ok_or_else(|| anyhow::anyhow!("tee pad for webrtc"))?;
    drop_upstream_reconfigure(&tee_src);
    let q_sink = q_webrtc
        .static_pad("sink")
        .ok_or_else(|| anyhow::anyhow!("q_webrtc sink"))?;

    // q → parse → dec → vconv → vcf → vp8 → pay → cf_rtp
    gst::Element::link_many(&[
        &q_webrtc, &parse_in, &dec, &vconv, &vcf, &vp8, &pay, &cf_rtp,
    ])?;

    // cf_rtp → webrtcbin.sink_%u
    let webrtc_sink = webrtc
        .request_pad_simple("sink_%u")
        .ok_or_else(|| anyhow::anyhow!("webrtc video sink pad"))?;
    let rtp_src = cf_rtp
        .static_pad("src")
        .ok_or_else(|| anyhow::anyhow!("cf_rtp src pad"))?;
    rtp_src.link(&webrtc_sink)?;

    // Audio: gemeinsamer Audio-Tee (Opus/RTP) → webrtcbin
    let audio_tee = pipeline
        .by_name("shared_audio_tee")
        .ok_or_else(|| anyhow::anyhow!("shared audio source not found"))?;
    let q_a = gst::ElementFactory::make("queue")
        .name(&format!("q_webrtc_a_{id}"))
        .build()?;
    let aconv = gst::ElementFactory::make("audioconvert")
        .name(&format!("webrtc_aconv_{id}"))
        .build()?;
    let ares = gst::ElementFactory::make("audioresample")
        .name(&format!("webrtc_ares_{id}"))
        .build()?;
    let vol = gst::ElementFactory::make("volume")
        .name(&format!("webrtc_volume_{id}"))
        .build()?;
    // Aktuelle Stream-Lautstärke übernehmen (Regler auf der Simulieren-Seite)
    vol.set_property("volume", gain);
    vol.set_property("mute", muted);
    let opus = gst::ElementFactory::make("opusenc")
        .name(&format!("webrtc_opus_{id}"))
        .build()?;
    let rtp = gst::ElementFactory::make("rtpopuspay")
        .name(&format!("webrtc_rtp_opus_{id}"))
        .build()?;
    rtp.set_property("pt", 111u32);
    let cf_a = gst::ElementFactory::make("capsfilter")
        .name(&format!("webrtc_a_caps_{id}"))
        .build()?;
    let a_caps = gst::Caps::builder("application/x-rtp")
        .field("media", &"audio")
        .field("encoding-name", &"OPUS")
        .field("payload", 111i32)
        .field("clock-rate", 48_000i32)
        .build();
    cf_a.set_property("caps", &a_caps);

    pipeline.add_many(&[&q_a, &aconv, &ares, &vol, &opus, &rtp, &cf_a])?;
    gst::Element::link_many(&[&q_a, &aconv, &ares, &vol, &opus, &rtp, &cf_a])?;
    let tee_audio_src = audio_tee
        .request_pad_simple("src_%u")
        .ok_or_else(|| anyhow::anyhow!("audio tee pad for webrtc"))?;
    drop_upstream_reconfigure(&tee_audio_src);
    let q_a_sink = q_a
        .static_pad("sink")
        .ok_or_else(|| anyhow::anyhow!("q_webrtc_a sink"))?;

    let webrtc_sink_a = webrtc
        .request_pad_simple("sink_%u")
        .ok_or_else(|| anyhow::anyhow!("webrtc audio sink pad"))?;
    let a_src = cf_a
        .static_pad("src")
        .ok_or_else(|| anyhow::anyhow!("cf_a src pad"))?;
    a_src.link(&webrtc_sink_a)?;

    // States mit Parent synchronisieren
    for e in [
        &q_webrtc, &parse_in, &dec, &vconv, &vcf, &vp8, &pay, &cf_rtp, &webrtc,
    ] {
        e.sync_state_with_parent()?;
    }
    for e in [&q_a, &aconv, &ares, &vol, &opus, &rtp, &cf_a] {
        e.sync_state_with_parent()?;
    }

    // Erst jetzt an die laufenden tees hängen: Ein verlinkter, aber noch gestoppter
    // Zweig lehnt die Sticky-Events (Caps/Segment) mit FLUSHING ab; tee reicht das bis
    // zur Videoquelle durch, deren Task pausiert – Aufnahme und alle Zuschauer frieren
    // ein (trat auf, wenn ein Zuschauer genau während des Session-Starts beitrat).
    tee_src.link(&q_sink)?;
    tee_audio_src.link(&q_a_sink)?;

    let elems = vec![
        q_webrtc,
        parse_in,
        dec,
        vconv,
        vcf,
        vp8,
        pay,
        cf_rtp,
        webrtc.clone(),
    ];
    let elems_audio: Vec<gst::Element> = vec![q_a, aconv, ares, vol, opus, rtp, cf_a];
    let tee_for_cleanup = tee.clone();
    let audio_tee_for_cleanup = audio_tee.clone();
    let pipeline_for_cleanup = pipeline.clone();
    let webrtc_for_cleanup = webrtc.clone();
    let cleanup = move || {
        glib::MainContext::default().invoke(move || {
            let _ = tee_for_cleanup.release_request_pad(&tee_src);
            let _ = audio_tee_for_cleanup.release_request_pad(&tee_audio_src);
            let _ = webrtc_for_cleanup.release_request_pad(&webrtc_sink);
            let _ = webrtc_for_cleanup.release_request_pad(&webrtc_sink_a);
            for e in elems.iter().rev() {
                let _ = e.set_state(gst::State::Null);
                let _ = pipeline_for_cleanup.remove(e);
            }
            for e in elems_audio.iter().rev() {
                let _ = e.set_state(gst::State::Null);
                let _ = pipeline_for_cleanup.remove(e);
            }
        });
    };

    Ok((webrtc, Box::new(cleanup)))
}
