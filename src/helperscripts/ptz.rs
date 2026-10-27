// src/helperscripts/ptz.rs
//
// Minimale Vendor-Abstraktion für Pan/Tilt/Zoom-Steuerung. Axis ist der erste
// (und bisher einzige) implementierte Treiber via VAPIX; weitere Hersteller
// können hier als zusätzliche match-Zweige ergänzt werden, ohne dass Aufrufer
// (Routen/Frontend) sich ändern müssen.

use std::fmt;
use std::time::Duration;

use crate::db::Camera;

#[derive(Debug)]
pub enum PtzError {
    /// Kamera-Marke hat (noch) keinen PTZ-Treiber.
    Unsupported,
    /// Kamera ist nicht als PTZ-fähig markiert oder hat keine VAPIX-Zugangsdaten.
    NotConfigured,
    /// Netzwerkfehler/Timeout beim Erreichen der Kamera.
    Network(String),
    /// Kamera hat mit einem Fehlerstatus geantwortet.
    Http(u16),
}

impl fmt::Display for PtzError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PtzError::Unsupported => write!(f, "PTZ für diese Kamera-Marke nicht unterstützt"),
            PtzError::NotConfigured => write!(f, "Kamera ist nicht für PTZ konfiguriert"),
            PtzError::Network(e) => write!(f, "Kamera nicht erreichbar: {e}"),
            PtzError::Http(code) => write!(f, "Kamera antwortete mit Status {code}"),
        }
    }
}

/// Bewegt die Kamera (Pan/Tilt/Zoom, jeweils -100..=100). `0,0,0` stoppt die Bewegung.
pub async fn ptz_move(cam: &Camera, pan: i32, tilt: i32, zoom: i32) -> Result<(), PtzError> {
    match cam.brand.as_str() {
        "axis" => axis_ptz_move(cam, pan, tilt, zoom).await,
        _ => Err(PtzError::Unsupported),
    }
}

async fn axis_ptz_move(cam: &Camera, pan: i32, tilt: i32, zoom: i32) -> Result<(), PtzError> {
    if cam.ptz_capable == 0 {
        return Err(PtzError::NotConfigured);
    }
    let (user, pass) = match (cam.vapix_username.as_deref(), cam.vapix_password.as_deref()) {
        (Some(u), Some(p)) if !u.is_empty() => (u, p),
        _ => return Err(PtzError::NotConfigured),
    };

    let host = url::Url::parse(&cam.url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .ok_or_else(|| PtzError::Network("Kamera-Host konnte nicht aus der Stream-URL gelesen werden".into()))?;
    let port = cam.vapix_port.unwrap_or(80);

    let vapix_url = format!(
        "http://{host}:{port}/axis-cgi/com/ptz.cgi?camera=1&continuouspantiltmove={pan},{tilt}&continuouszoommove={zoom}"
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|e| PtzError::Network(e.to_string()))?;

    let resp = client
        .get(&vapix_url)
        .basic_auth(user, Some(pass))
        .send()
        .await
        .map_err(|e| PtzError::Network(e.to_string()))?;

    if resp.status().is_success() {
        Ok(())
    } else {
        Err(PtzError::Http(resp.status().as_u16()))
    }
}
