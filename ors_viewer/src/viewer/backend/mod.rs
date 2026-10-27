use anyhow::Result;

#[derive(Debug)]
pub enum PixelFmt {
    Rgba,
}

pub struct DecodedFrame {
    pub ts_us: u64,
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>, // RGBA
}

pub struct AudioBuffer {
    pub ts_us: u64,
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

pub trait DecoderBackend: Send {
    fn push_video(&mut self, ts_us: u64, annexb: &[u8]) -> Result<Option<DecodedFrame>>;
    fn push_audio(&mut self, ts_us: u64, aac: &[u8]) -> Result<Option<AudioBuffer>>;
}

#[cfg(feature = "ffmpeg")]
pub mod ffmpeg;
