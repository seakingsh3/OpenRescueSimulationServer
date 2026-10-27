use anyhow::Result;
use pixels::{Pixels, SurfaceTexture};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use winit::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop, EventLoopBuilder},
    window::WindowBuilder,
};

#[cfg(target_os = "linux")]
use winit::platform::{wayland::EventLoopBuilderExtWayland, x11::EventLoopBuilderExtX11};

#[derive(Clone)]
pub struct VideoWindow {
    inner: Arc<Inner>,
}
struct Inner {
    visible: Mutex<bool>,
    // shared last frame
    last: Mutex<Option<FrameBuf>>,
}
struct FrameBuf {
    w: u32, h: u32,
    data: Vec<u8>, // RGBA
}

impl VideoWindow {
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: Arc::new(Inner {
                visible: Mutex::new(false),
                last: Mutex::new(None),
            })
        })
    }

    pub fn show(&self) {
        let mut vis = self.inner.visible.lock().unwrap();
        if *vis { return; }
        *vis = true;

        let inner = self.inner.clone();
        std::thread::spawn(move || {
            // any_thread on Linux
            #[cfg(target_os = "linux")]
            let event_loop = {
                let mut b = EventLoopBuilder::new();
                EventLoopBuilderExtWayland::with_any_thread(&mut b, true);
                EventLoopBuilderExtX11::with_any_thread(&mut b, true);
                b.build()
            };
            #[cfg(not(target_os = "linux"))]
            let event_loop = EventLoop::new();

            let window = WindowBuilder::new()
                .with_title("ORS Viewer")
                .with_inner_size(LogicalSize::new(1280.0, 720.0))
                .build(&event_loop)
                .expect("window");

            let size = window.inner_size();
            let surface = SurfaceTexture::new(size.width, size.height, &window);
            let mut pixels = Pixels::new(1280, 720, surface).expect("pixels");

            event_loop.run(move |event, _target, control_flow| {
                *control_flow = ControlFlow::Poll;

                match event {
                    Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                        *control_flow = ControlFlow::Exit;
                    }
                    Event::RedrawRequested(_) => {
                        // draw from last frame if any, else test pattern
                        let mut guard = inner.last.lock().unwrap();
                        if let Some(f) = guard.as_ref() {
                            // center letterbox, simple: scale off, just blit cropped if mismatch
                            let frame = pixels.frame_mut();
                            let w = f.w.min(1280);
                            let h = f.h.min(720);
                            // naive copy top-left (keep simple for now)
                            for y in 0..h {
                                let src = &f.data[(y*f.w*4) as usize .. (y*f.w*4 + w*4) as usize];
                                let dst = &mut frame[(y*1280*4) as usize .. (y*1280*4 + w*4) as usize];
                                dst.copy_from_slice(src);
                            }
                        } else {
                            // fallback test pattern
                            let frame = pixels.frame_mut();
                            for (i, px) in frame.chunks_exact_mut(4).enumerate() {
                                let x = (i % 1280) as u8;
                                let y = (i / 1280) as u8;
                                px.copy_from_slice(&[x, y, 64, 255]);
                            }
                        }
                        let _ = pixels.render();
                    }
                    Event::MainEventsCleared => {
                        window.request_redraw();
                    }
                    _ => {}
                }
            });
        });
    }

    pub fn hide(&self) {
        let mut vis = self.inner.visible.lock().unwrap();
        *vis = false;
        // minimal: Fenster-Thread beendet bei Close. (später: proper hide)
    }

    /// Push a new RGBA frame (w*h*4). Drops silently if overflowing.
    pub fn render_rgba(&self, rgba: Vec<u8>, w: u32, h: u32) {
        let mut guard = self.inner.last.lock().unwrap();
        *guard = Some(FrameBuf { w, h, data: rgba });
    }

    pub fn tick(&self) {}
}
