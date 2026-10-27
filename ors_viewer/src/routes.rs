use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use crate::AppState;

#[derive(Serialize)]
pub struct Status {
    playing: bool,
    stream_url: String,
    volume: u8, // 0..100
    muted: bool,
}

pub async fn status(State(s): State<AppState>) -> Json<Status> {
    let playing = s.ctrl.playing();
    let stream_url = s.stream_url.read().await.clone();
    let (vol, muted) = (s.ctrl.volume_level(), s.ctrl.muted());
    Json(Status { playing, stream_url, volume: vol, muted })
}

pub async fn play(State(s): State<AppState>) {
    s.ctrl.set_playing(true);
    s.ctrl.mute(false);
    s.player.show_window().await;
}

pub async fn stop(State(s): State<AppState>) {
    s.ctrl.set_playing(false);
    s.ctrl.mute(true);
    s.player.hide_window().await;
}

#[derive(Deserialize)]
pub struct SetStreamReq { stream_url: String }
pub async fn set_stream(State(s): State<AppState>, Json(req): Json<SetStreamReq>) {
    *s.stream_url.write().await = req.stream_url.clone();
    s.player.set_stream(&req.stream_url).await;
}

pub async fn get_volume(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "level": s.ctrl.volume_level(), "muted": s.ctrl.muted() }))
}
pub async fn volume_up(State(s): State<AppState>) { s.ctrl.volume_up_db(2.0); }
pub async fn volume_down(State(s): State<AppState>) { s.ctrl.volume_up_db(-2.0); }

#[derive(Deserialize)]
pub struct MuteReq { on: Option<bool> }
pub async fn volume_mute(State(s): State<AppState>, Json(req): Json<MuteReq>) {
    s.ctrl.mute(req.on.unwrap_or(true));
}

#[derive(Deserialize)]
pub struct SetReq { level: u8 } // 0..100
pub async fn volume_set(State(s): State<AppState>, Json(req): Json<SetReq>) {
    s.ctrl.set_volume_level(req.level);
}
