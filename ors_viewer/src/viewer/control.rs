use std::sync::{Arc, atomic::{AtomicBool, AtomicU8, Ordering}};

#[derive(Clone)]
pub struct Control {
    playing: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    level_0_100: Arc<AtomicU8>,
}

impl Control {
    pub fn new() -> Self {
        Self {
            playing: Arc::new(AtomicBool::new(false)),
            muted: Arc::new(AtomicBool::new(true)),
            level_0_100: Arc::new(AtomicU8::new(70)), // default ~ -9 dB
        }
    }
    pub fn playing(&self) -> bool { self.playing.load(Ordering::SeqCst) }
    pub fn set_playing(&self, on: bool) { self.playing.store(on, Ordering::SeqCst) }

    pub fn muted(&self) -> bool { self.muted.load(Ordering::SeqCst) }
    pub fn mute(&self, on: bool) { self.muted.store(on, Ordering::SeqCst) }

    pub fn volume_level(&self) -> u8 { self.level_0_100.load(Ordering::SeqCst) }
    pub fn set_volume_level(&self, lvl: u8) { self.level_0_100.store(lvl.min(100), Ordering::SeqCst) }

    /// +dB relative; negative für leiser
    pub fn volume_up_db(&self, delta_db: f32) {
        // 0..100 -> dB in [-60..0]
        let cur = self.volume_level() as f32;
        let mut db = -60.0 + (cur/100.0)*60.0;
        db = (db + delta_db).clamp(-60.0, 0.0);
        let lvl = ((db + 60.0)/60.0*100.0).round() as u8;
        self.set_volume_level(lvl);
    }

    /// Konvertiert 0..100 Level in linearen Gain
    pub fn gain_linear(&self) -> f32 {
        let lvl = self.volume_level() as f32;
        let db = -60.0 + (lvl/100.0)*60.0;
        10f32.powf(db/20.0) // -60..0 dB → 0.001..1.0
    }
}
