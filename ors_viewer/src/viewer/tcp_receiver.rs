use super::{AvPacket, AvKind};
use bytes::{Bytes, BytesMut};
use tokio::{net::TcpStream, io::AsyncReadExt, sync::mpsc};

/// Verbindet zu tcp://host:port und liest Frames:
/// [kind:u8][ts_us:u64][len:u32][payload]
pub async fn connect(url: &str) -> anyhow::Result<(mpsc::Receiver<AvPacket>, mpsc::Receiver<AvPacket>)> {
    // url: tcp://host:port
    let addr = url.strip_prefix("tcp://").unwrap_or(url);
    let mut sock = TcpStream::connect(addr).await?;
    // optional: Magic "ORS1" am Anfang prüfen
    let mut magic = [0u8;4];
    if let Err(_) = sock.read_exact(&mut magic).await { /* tolerant */ }
    // Kanäle
    let (v_tx, v_rx) = mpsc::channel::<AvPacket>(64);
    let (a_tx, a_rx) = mpsc::channel::<AvPacket>(256);

    tokio::spawn(async move {
        let mut buf = BytesMut::with_capacity(1024*1024);
        loop {
            // Header lesen
            let mut hdr = [0u8; 1+8+4];
            if sock.read_exact(&mut hdr).await.is_err() { break; }
            let kind = hdr[0];
            let ts_us = u64::from_le_bytes(hdr[1..9].try_into().unwrap());
            let len = u32::from_le_bytes(hdr[9..13].try_into().unwrap()) as usize;

            buf.resize(len, 0);
            if sock.read_exact(&mut buf).await.is_err() { break; }
            let pkt = AvPacket {
                kind: if kind==0 { AvKind::Video } else { AvKind::Audio },
                ts_us,
                data: Bytes::from(buf.to_vec()),
            };
            let _ = match pkt.kind {
                AvKind::Video => v_tx.try_send(pkt),
                AvKind::Audio => a_tx.try_send(pkt),
            };
        }
    });

    Ok((v_rx, a_rx))
}
