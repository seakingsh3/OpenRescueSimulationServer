// src/helperscripts/kamera.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum StreamKind {
    Main,
    Sub,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Protocol {
    Rtsp,
    Rtmp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CameraVendor {
    Hikvision,
    Dahua,
    Axis,
    Reolink,
    Uniview,
    Generic,
    Onvif,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraSimple {
    pub host: String,
    pub port: Option<u16>,
    pub username: String,
    pub password: String,
    pub channel: Option<u8>,

    /// Main/Sub
    pub stream: Option<StreamKind>,

    /// explizit gewünschtes Protokoll
    pub protocol: Option<Protocol>,

    /// für Axis (z. B. “default”)
    pub axis_profile: Option<String>,

    /// generischer Pfad (z. B. “/h264.sdp”, “/live/stream”)
    pub generic_path: Option<String>,
}

fn default_rtsp_port(port: Option<u16>) -> u16 {
    port.unwrap_or(554)
}
fn default_rtmp_port(port: Option<u16>) -> u16 {
    port.unwrap_or(1935)
}

fn auth_prefix(user: &str, pass: &str) -> String {
    if user.is_empty() {
        "".into()
    } else if pass.is_empty() {
        format!("{}@", user)
    } else {
        format!("{}:{}@", user, pass)
    }
}

// --- konkrete URL-Builder -----------------------------------------------------

fn build_rtsp(cam: &CameraSimple, vendor: CameraVendor) -> String {
    let host = cam.host.trim();
    let ap = auth_prefix(&cam.username, &cam.password);
    let ch = cam.channel.unwrap_or(1);
    let port = default_rtsp_port(cam.port);
    let sk = cam.stream.unwrap_or(StreamKind::Main);

    let path = match vendor {
        CameraVendor::Hikvision | CameraVendor::Uniview => {
            // Hik/UV: Streaming/Channels/{ch}01 (Main) bzw. {ch}02 (Sub)
            let suffix = match sk {
                StreamKind::Main => "01",
                StreamKind::Sub => "02",
            };
            format!("/Streaming/Channels/{}{}", ch, suffix)
        }
        CameraVendor::Dahua => {
            // Dahua: /cam/realmonitor?channel={ch}&subtype=0|1
            let subtype = match sk {
                StreamKind::Main => 0,
                StreamKind::Sub => 1,
            };
            format!("/cam/realmonitor?channel={}&subtype={}", ch, subtype)
        }
        CameraVendor::Axis => {
            // Axis: /axis-media/media.amp?streamprofile={axis_profile}
            let prof = cam.axis_profile.as_deref().unwrap_or("default");
            format!("/axis-media/media.amp?streamprofile={}", prof)
        }
        CameraVendor::Reolink => match sk {
            StreamKind::Main => "/h264Preview_01_main".into(),
            StreamKind::Sub => "/h264Preview_01_sub".into(),
        },
        CameraVendor::Onvif | CameraVendor::Generic => {
            // generisch oder ONVIF: nutze custom path oder fallback
            cam.generic_path
                .as_deref()
                .unwrap_or("/h264.sdp")
                .to_string()
        }
    };

    format!("rtsp://{}{}:{}{}", ap, host, port, path)
}

fn build_rtmp(cam: &CameraSimple) -> String {
    // generische RTMP-URL (Kameras liefern selten nativ RTMP – ist meist für NVR/Server)
    let host = cam.host.trim();
    let port = default_rtmp_port(cam.port);
    // Pfad – wenn generisch vorhanden, sonst ein Standard
    let path = cam
        .generic_path
        .as_deref()
        .unwrap_or("/live/stream")
        .trim_start_matches('/');
    format!("rtmp://{}:{}/{}", host, port, path)
}

// --- public API ---------------------------------------------------------------

/// Für GUI, Routing etc.: Liste der unterstützten Hersteller.
pub fn supported_vendors() -> Vec<CameraVendor> {
    vec![
        CameraVendor::Hikvision,
        CameraVendor::Dahua,
        CameraVendor::Axis,
        CameraVendor::Reolink,
        CameraVendor::Uniview,
        CameraVendor::Generic,
        CameraVendor::Onvif,
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltUris {
    pub rtsp: Option<String>,
    pub rtmp: Option<String>,
}

/// Baut – je nach gewünschten/angegebenen Feldern – RTSP/RTMP.
/// Wenn `cam.protocol` gesetzt ist, wird nur dieses Protokoll befüllt.
/// Ohne gesetztes `protocol` wird RTSP (Generic) immer gebaut und RTMP leer gelassen.
pub fn build_uris(cam: &CameraSimple) -> BuiltUris {
    let vendor = CameraVendor::Generic; // API-Signatur in routes.rs übergibt keinen Vendor
    match cam.protocol.unwrap_or(Protocol::Rtsp) {
        Protocol::Rtsp => BuiltUris {
            rtsp: Some(build_rtsp(cam, vendor)),
            rtmp: None,
        },
        Protocol::Rtmp => BuiltUris {
            rtsp: None,
            rtmp: Some(build_rtmp(cam)),
        },
    }
}

/// Exakter Builder mit Vendor – für die GUI.
pub fn build_simple(cam: &CameraSimple, vendor: CameraVendor) -> Result<String, String> {
    Ok(match cam.protocol.unwrap_or(Protocol::Rtsp) {
        Protocol::Rtsp => build_rtsp(cam, vendor),
        Protocol::Rtmp => build_rtmp(cam),
    })
}
