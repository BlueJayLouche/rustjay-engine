//! Mapped-LED output — drives a recovered `ledmap.json` over sACN from the
//! rendered output frame.
//!
//! A CPU-path output like NDI/V4L2: [`OutputManager`](super::OutputManager)
//! harvests the readback frame and hands it to [`LedOutput::submit`], which
//! samples each LED's `(u,v)` ([`PointMap`]) and streams the result via a
//! `rustjay-lighting` `DmxSender`.

use std::path::Path;

use rustjay_ledmap::PointMap;
use rustjay_lighting::{Dest, DmxSender, SacnTransport};

/// A live mapped-LED sACN output.
pub struct LedOutput {
    map: PointMap,
    sender: DmxSender,
}

impl LedOutput {
    /// Load a `ledmap.json` and start an sACN sender (multicast, `priority`).
    pub fn new(map_path: &Path, priority: u8) -> anyhow::Result<Self> {
        let map = PointMap::load(map_path)?;
        if map.is_empty() {
            anyhow::bail!("ledmap '{}' has no drivable LEDs", map_path.display());
        }
        let transport = SacnTransport::new(Dest::Multicast, priority, "rustjay-engine")?;
        let sender = DmxSender::spawn(Box::new(transport), 44.0);
        Ok(Self { map, sender })
    }

    /// Number of LEDs being driven.
    pub fn led_count(&self) -> usize {
        self.map.len()
    }

    /// Place the LED layout into a canvas region (move/scale/corner-pin). `quad`
    /// is `[TL, TR, BR, BL]` in `[0,1]`; `None` samples the whole canvas.
    pub fn set_placement(&mut self, quad: Option<[[f32; 2]; 4]>) {
        self.map.set_placement(quad);
    }

    /// Sample a BGRA8 frame and stream it to the strip.
    pub fn submit(&self, bgra: &[u8], width: u32, height: u32) {
        self.sender
            .submit(self.map.sample(bgra, width as usize, height as usize));
    }

    /// Send an all-off frame (call before dropping to clear the strip).
    pub fn blackout(&self) {
        self.sender.submit(self.map.blackout());
    }
}
