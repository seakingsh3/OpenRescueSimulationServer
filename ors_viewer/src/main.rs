use axum::{routing::{get, post}, Router};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::RwLock;
use tracing_subscriber::EnvFilter;

mod routes;
mod viewer;

#[derive(Clone)]
pub struct AppState {
    pub stream_url: Arc<RwLock<String>>,
    pub ctrl: viewer::control::Control,
    pub player: viewer::Player,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .init();

    let stream_url = Arc::new(RwLock::new("tcp://127.0.0.1:5005".to_string()));
    let ctrl = viewer::control::Control::new();
    let player = viewer::Player::new(ctrl.clone()).await?;

    let state = AppState { stream_url, ctrl, player };

    let app = Router::new()
        .route("/status", get(routes::status))
        .route("/play",   post(routes::play))
        .route("/stop",   post(routes::stop))
        .route("/set_stream", post(routes::set_stream))
        .route("/volume", get(routes::get_volume))
        .route("/volume/up", post(routes::volume_up))
        .route("/volume/down", post(routes::volume_down))
        .route("/volume/mute", post(routes::volume_mute))
        .route("/volume/set",  post(routes::volume_set))
        .with_state(state.clone());

    let addr: SocketAddr = "0.0.0.0:5000".parse().unwrap();
    tracing::info!("HTTP listening on http://{addr}");
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}
