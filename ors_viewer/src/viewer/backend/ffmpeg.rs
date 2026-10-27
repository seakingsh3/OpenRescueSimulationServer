use anyhow::{Context, Result};
use ffmpeg_next as ffmpeg;

use super::DecodedFrame;

pub struct FfmpegDecoder {
    dec: ffmpeg::codec::decoder::Video,                 // ctx entfernt
    sws: Option<ffmpeg::software::scaling::Context>,
    w: u32,
    h: u32,
}

impl FfmpegDecoder {
    pub fn new_h264() -> Result<Self> {
        ffmpeg::init().ok();

        // nur als Verfügbarkeitscheck:
        let _ = ffmpeg::codec::decoder::find(ffmpeg::codec::Id::H264)
            .context("h264 codec not found")?;

        // Context sofort in Decoder konsumieren – wir speichern nur den Decoder
        let dec = ffmpeg::codec::context::Context::new()
            .decoder()
            .video()?;

        Ok(Self { dec, sws: None, w: 0, h: 0 })
    }

    pub fn push_annexb(&mut self, ts_us: u64, data: &[u8]) -> Result<Option<DecodedFrame>> {
        let mut out: Option<DecodedFrame> = None;

        let mut pkt = ffmpeg::codec::packet::Packet::copy(data);
        pkt.set_pts(Some(ts_us as i64));
        pkt.set_dts(Some(ts_us as i64));

        self.dec.send_packet(&pkt)?;

        loop {
            let mut frame = ffmpeg::util::frame::video::Video::empty();
            match self.dec.receive_frame(&mut frame) {
                Ok(()) => {
                    let fw = frame.width() as u32;
                    let fh = frame.height() as u32;

                    if self.sws.is_none() || self.w != fw || self.h != fh {
                        self.sws = Some(ffmpeg::software::scaling::Context::get(
                            frame.format(),
                            frame.width(),
                            frame.height(),
                            ffmpeg::format::Pixel::RGBA,
                            frame.width(),
                            frame.height(),
                            ffmpeg::software::scaling::flag::Flags::BILINEAR,
                        )?);
                        self.w = fw;
                        self.h = fh;
                    }

                    let mut dst = ffmpeg::util::frame::video::Video::new(
                        ffmpeg::format::Pixel::RGBA,
                        frame.width(),
                        frame.height(),
                    );
                    if let Some(sws) = &mut self.sws {
                        sws.run(&frame, &mut dst)?;
                    }

                    let stride = dst.stride(0) as usize;
                    let data0 = dst.data(0);
                    let w = fw as usize;
                    let h = fh as usize;
                    let mut rgba = vec![0u8; w * h * 4];
                    for y in 0..h {
                        let src = &data0[y * stride .. y * stride + w * 4];
                        let dstl = &mut rgba[y * w * 4 .. (y + 1) * w * 4];
                        dstl.copy_from_slice(src);
                    }

                    out = Some(DecodedFrame { ts_us, w: fw, h: fh, data: rgba });
                }
                Err(e) => {
                    if e == ffmpeg::Error::Eof { break; }
                    break; // EAGAIN/sonstige: kein weiterer Frame
                }
            }
        }

        Ok(out)
    }
}
