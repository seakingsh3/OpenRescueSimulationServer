use crate::viewer::control::Control;
use anyhow::Result;

#[cfg(feature = "audio")]
mod real {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    pub struct AudioSink {
        _stream: cpal::Stream,
        _ctrl: Control,
    }

    impl AudioSink {
        pub fn new(ctrl: Control) -> Result<Self> {
            let host = cpal::default_host();
            let device = host
                .default_output_device()
                .ok_or_else(|| anyhow::anyhow!("no output device"))?;
            let config = device.default_output_config()?;

            let ctrl_clone = ctrl.clone();

            let stream = match config.sample_format() {
                cpal::SampleFormat::F32 => device.build_output_stream(
                    &config.into(),
                    move |output: &mut [f32], _| {
                        let gain = if ctrl_clone.muted() { 0.0 } else { ctrl_clone.gain_linear() };
                        for s in output { *s = 0.0 * gain; } // TODO: PCM-Ringbuffer einspeisen
                    },
                    move |err| eprintln!("audio err: {err}"),
                    None,
                )?,
                _ => anyhow::bail!("unsupported sample format"),
            };
            stream.play()?;
            Ok(Self { _stream: stream, _ctrl: ctrl })
        }
    }
}

#[cfg(feature = "audio")]
pub use real::AudioSink;

#[cfg(not(feature = "audio"))]
pub struct AudioSink;

#[cfg(not(feature = "audio"))]
impl AudioSink {
    pub fn new(_ctrl: Control) -> Result<Self> { Ok(Self) }
}
