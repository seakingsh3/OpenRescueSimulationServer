use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::{broadcast, broadcast::Sender, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub layout: String,
    pub sources: Vec<Option<String>>, // bis 6 Slots
}

pub fn new_status() -> Arc<Mutex<Status>> {
    Arc::new(Mutex::new(Status {
        layout: "2x2".to_string(),
        sources: vec![None, None, None, None, None, None],
    }))
}

#[derive(Clone)]
pub struct AppCtx {
    pub db: SqlitePool,
    pub status: Arc<Mutex<Status>>,
    pub tx: Sender<Status>,
    // WICHTIG: Arc<Mutex<...>> damit Clone ableitbar ist
    pub pipeline: Arc<Mutex<crate::pipeline::PipelineManager>>,

    // 🆕 alle aktiven WebSocket-Verbindungen pro Viewer-ID
    pub viewer_channels: Arc<Mutex<HashMap<i64, Vec<mpsc::UnboundedSender<String>>>>>,

    // 🆕 Broadcast an UIs (z.B. Pre-Start Toast)
    pub ui_broadcast: Arc<Mutex<Vec<mpsc::UnboundedSender<String>>>>,

    // Auth-Secret (JWT HMAC)
    pub auth_secret: Arc<String>,

    // Signal zum Beenden aller aktiven WebRTC-WS (harte Session-Neustarts)
    pub webrtc_shutdown: broadcast::Sender<()>,
}
