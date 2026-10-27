pub mod control;
pub mod tcp_receiver;
pub mod audio_sink;
pub mod video_renderer;
pub mod backend;
use std::sync::mpsc as std_mpsc;

use anyhow::Result;
use bytes::Bytes;
use control::Control;
use tokio::sync::mpsc;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvKind { Video, Audio }

#[derive(Debug, Clone)]
pub struct AvPacket { pub kind: AvKind, pub ts_us: u64, pub data: Bytes }

#[derive(Clone)]
pub struct Player {
    ctrl: Control,
    cmd_tx: mpsc::UnboundedSender<PlayerCmd>,
}

enum PlayerCmd { SetStream(String), Show, Hide }

impl Player {
    pub async fn new(ctrl: Control) -> Result<Self> {
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<PlayerCmd>();

        tokio::spawn({
    // Channels: Tokio → DecoderThread (Annex-B-Pakete), DecoderThread → Tokio (RGBA-Frames)
    #[cfg(feature = "ffmpeg")]
    let (tx_to_dec, rx_to_dec) = std_mpsc::channel::<AvPacket>();
    #[cfg(feature = "ffmpeg")]
    let (tx_from_dec, rx_from_dec) = std_mpsc::channel::<backend::DecodedFrame>();

    // Decoder-Thread starten (kein Send erforderlich)
    #[cfg(feature = "ffmpeg")]
    std::thread::spawn(move || {
        use backend::ffmpeg::FfmpegDecoder;
        let mut dec = FfmpegDecoder::new_h264().expect("ffmpeg h264");
        while let Ok(pkt) = rx_to_dec.recv() {
            if pkt.kind == AvKind::Video {
                if let Ok(Some(frame)) = dec.push_annexb(pkt.ts_us, &pkt.data) {
                    let _ = tx_from_dec.send(frame);
                }
            }
        }
    });

    async move {
        let video = video_renderer::VideoWindow::new().expect("video window");
        #[cfg(feature = "audio")]
        {
            // später: Audio im eigenen Thread mit Ringpuffer
            // let _ = audio_sink::AudioSink::new(ctrl.clone());
        }

        let mut vid_rx: Option<mpsc::Receiver<AvPacket>> = None;
        let mut aud_rx: Option<mpsc::Receiver<AvPacket>> = None;

        loop {
            // 1) Kommandos
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    PlayerCmd::SetStream(url) => {
                        info!("PlayerCmd::SetStream({})", url);
                        match tcp_receiver::connect(&url).await {
                            Ok((v, a)) => { vid_rx = Some(v); aud_rx = Some(a); }
                            Err(e) => warn!("tcp connect failed: {e:?}"),
                        }
                    }
                    PlayerCmd::Show => { info!("PlayerCmd::Show"); video.show(); }
                    PlayerCmd::Hide => { info!("PlayerCmd::Hide"); video.hide(); }
                }
            }

            // 2) Video-Pakete aus TCP holen und an Decoder-Thread schicken
            if let Some(r) = vid_rx.as_mut() {
                if let Ok(pkt) = r.try_recv() {
                    #[cfg(feature = "ffmpeg")]
                    { let _ = tx_to_dec.send(pkt); }
                    #[cfg(not(feature = "ffmpeg"))]
                    { video.tick(); }
                }
            }

            // 3) RGBA-Frames vom Decoder-Thread abholen und rendern
            #[cfg(feature = "ffmpeg")]
            while let Ok(frame) = rx_from_dec.try_recv() {
                video.render_rgba(frame.data, frame.w, frame.h);
            }

            // 4) Audio (später)
            if let Some(r) = aud_rx.as_mut() {
                if let Ok(_pkt) = r.try_recv() {
                    // TODO: AAC → PCM → AudioSink
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
});


        Ok(Self { ctrl, cmd_tx })
    }

    pub async fn set_stream(&self, url: &str) { let _ = self.cmd_tx.send(PlayerCmd::SetStream(url.to_string())); }
    pub async fn show_window(&self) { let _ = self.cmd_tx.send(PlayerCmd::Show); }
    pub async fn hide_window(&self) { let _ = self.cmd_tx.send(PlayerCmd::Hide); }
}
