//! Sequential single-LED flash calibration (Milestone 1).
//!
//! Drives one LED on per step in wiring order, captures a frame per step, and
//! records the brightest blob as that LED's position. Frame ↔ index
//! correspondence is exact because this controller owns both ends.
//!
//! Output reuses `rustjay-lighting`: [`SequentialCalibrator::dmx_frame`] packs
//! the current step into a [`DmxFrame`] via `pack_fixtures`; the app submits it
//! to a `DmxSender` (sACN). The app loop is: `dmx_frame()` → submit → settle →
//! capture luma → [`record`](SequentialCalibrator::record) → repeat; then
//! [`is_done`](SequentialCalibrator::is_done) → [`finish`](SequentialCalibrator::finish).
//!
//! Gray-code (scales as log₂N, enables uploaded-video ingestion) is Milestone 3
//! and slots in beside this as an alternate controller.

use rustjay_lighting::{pack_fixtures, DmxFrame, DMX_UNIVERSE_SIZE};

use super::detect::brightest_blob;
use super::format::{Led, LedMap, Source, Space, LEDMAP_VERSION};

/// Drives a sequential flash and accumulates per-LED centroids.
pub struct SequentialCalibrator {
    count: u32,
    order: String,
    footprint: usize,
    start_universe: u16,
    start_channel: u16,
    on_level: u8,
    threshold: u8,
    image_wh: [u32; 2],
    /// Index of the LED to light on the *next* `dmx_frame()` / `record()`.
    current: u32,
    /// Per-LED `(x, y, conf)` in image pixels; `None` until recorded.
    centroids: Vec<Option<(f32, f32, f32)>>,
}

impl SequentialCalibrator {
    /// `count` LEDs, `order` color string (e.g. `"GRB"`), patched from
    /// `start_universe`/`start_channel` (1-based channel). `on_level` is the
    /// value written to each channel of the lit LED; `threshold` is the luma
    /// cutoff for blob detection.
    pub fn new(
        count: u32,
        order: impl Into<String>,
        start_universe: u16,
        start_channel: u16,
        on_level: u8,
        threshold: u8,
    ) -> Self {
        let order = order.into();
        let footprint = order.len().max(1);
        Self {
            count,
            order,
            footprint,
            start_universe,
            start_channel,
            on_level,
            threshold,
            image_wh: [0, 0],
            current: 0,
            centroids: vec![None; count as usize],
        }
    }

    /// The LED index that the next [`dmx_frame`](Self::dmx_frame) lights.
    pub fn current_index(&self) -> u32 {
        self.current
    }

    /// Total LED count being calibrated.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// True once every LED has had its frame recorded.
    pub fn is_done(&self) -> bool {
        self.current >= self.count
    }

    /// Fixture-major channel buffer for this step: all zero except the current
    /// LED's `footprint` channels set to `on_level`. Length `count * footprint`.
    /// Empty once [`is_done`](Self::is_done).
    ///
    /// Mostly an internal/testing seam; the app drives with
    /// [`dmx_frame`](Self::dmx_frame).
    pub fn pattern(&self) -> Vec<u8> {
        let mut buf = vec![0u8; self.count as usize * self.footprint];
        if !self.is_done() {
            let base = self.current as usize * self.footprint;
            for c in &mut buf[base..base + self.footprint] {
                *c = self.on_level;
            }
        }
        buf
    }

    /// The current step packed into a [`DmxFrame`] ready to submit to a
    /// `rustjay_lighting::DmxSender`. Patch wrapping (no fixture split across a
    /// universe) is handled by `pack_fixtures`.
    pub fn dmx_frame(&self) -> DmxFrame {
        let mut frame = DmxFrame::new();
        pack_fixtures(
            &mut frame,
            self.footprint,
            &self.pattern(),
            self.start_universe,
            self.start_channel,
        );
        frame
    }

    /// An all-zero [`DmxFrame`] spanning the same universes — submit on stop to
    /// turn the strip off.
    pub fn blackout_frame(&self) -> DmxFrame {
        let mut frame = DmxFrame::new();
        let zeros = vec![0u8; self.count as usize * self.footprint];
        pack_fixtures(&mut frame, self.footprint, &zeros, self.start_universe, self.start_channel);
        frame
    }

    /// Record the captured frame for the current LED and advance.
    ///
    /// `luma` is row-major `w*h`. The brightest blob (if any clears `threshold`)
    /// becomes this LED's centroid; otherwise it is left undetected (`conf 0`).
    /// No-op once [`is_done`](Self::is_done).
    pub fn record(&mut self, luma: &[u8], w: usize, h: usize) {
        if self.is_done() {
            return;
        }
        self.image_wh = [w as u32, h as u32];
        if let Some(b) = brightest_blob(luma, w, h, self.threshold) {
            let conf = (b.weight as f32 / (b.area as f32 * 255.0)).min(1.0);
            self.centroids[self.current as usize] = Some((b.x, b.y, conf));
        }
        self.current += 1;
    }

    /// Build the [`LedMap`]. Detected LEDs are normalized to `[0,1]`; undetected
    /// LEDs land at `(0,0)` with `conf 0.0` so a consumer or fixup UI can flag
    /// them. `captured` is an RFC3339 timestamp string supplied by the caller.
    pub fn finish(&self, captured: impl Into<String>) -> LedMap {
        let [iw, ih] = self.image_wh;
        let (fw, fh) = (iw.max(1) as f32, ih.max(1) as f32);

        let leds = (0..self.count)
            .map(|i| {
                let (u, v, conf) = match self.centroids[i as usize] {
                    Some((x, y, c)) => (x / fw, y / fh, c),
                    None => (0.0, 0.0, 0.0),
                };
                let (universe, channel) = self.address_of(i);
                Led { i, u, v, w: None, universe, channel, order: self.order.clone(), conf }
            })
            .collect();

        LedMap {
            version: LEDMAP_VERSION,
            space: Space::Canvas2d,
            source: Source {
                tool: "rustjay-ledmap".into(),
                captured: captured.into(),
                image_wh: self.image_wh,
            },
            leds,
        }
    }

    /// DMX `(universe, 1-based channel)` for LED `i` — for annotating the export.
    /// Mirrors `rustjay_lighting::pack_fixtures`: advance by `footprint`, wrap to
    /// the next universe without splitting a fixture.
    fn address_of(&self, i: u32) -> (u16, u16) {
        let mut universe = self.start_universe;
        let mut ch = (self.start_channel.max(1) as usize) - 1; // 0-based
        for _ in 0..i {
            if ch + self.footprint > DMX_UNIVERSE_SIZE {
                universe = universe.wrapping_add(1);
                ch = 0;
            }
            ch += self.footprint;
        }
        if ch + self.footprint > DMX_UNIVERSE_SIZE {
            universe = universe.wrapping_add(1);
            ch = 0;
        }
        (universe, (ch + 1) as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_lights_one_fixture() {
        let mut cal = SequentialCalibrator::new(3, "GRB", 1, 1, 255, 32);
        assert_eq!(cal.pattern(), vec![255, 255, 255, 0, 0, 0, 0, 0, 0]);
        cal.record(&[0u8; 4], 2, 2);
        assert_eq!(cal.pattern(), vec![0, 0, 0, 255, 255, 255, 0, 0, 0]);
    }

    /// The packed DmxFrame lights the current fixture's channels in universe 1.
    #[test]
    fn dmx_frame_packs_current_fixture() {
        let cal = SequentialCalibrator::new(3, "GRB", 1, 1, 255, 32);
        let frame = cal.dmx_frame();
        let u1 = frame.get(1).expect("universe 1 written");
        assert_eq!(&u1[0..3], &[255, 255, 255]);
        assert_eq!(&u1[3..6], &[0, 0, 0]);
    }

    #[test]
    fn records_and_normalizes_centroids() {
        let (w, h) = (10, 10);
        let mut cal = SequentialCalibrator::new(2, "GRB", 1, 1, 255, 32);

        let mut f0 = vec![0u8; w * h];
        f0[w + 1] = 255; f0[w + 2] = 255;
        cal.record(&f0, w, h);

        let mut f1 = vec![0u8; w * h];
        f1[8 * w + 7] = 255; f1[8 * w + 8] = 255;
        cal.record(&f1, w, h);

        assert!(cal.is_done());
        let map = cal.finish("2026-06-22T00:00:00Z");
        assert_eq!(map.leds.len(), 2);
        assert!(map.leds[0].u < 0.4 && map.leds[0].v < 0.4);
        assert!(map.leds[1].u > 0.6 && map.leds[1].v > 0.6);
        assert!(map.leds[0].conf > 0.0);
    }

    #[test]
    fn undetected_led_is_flagged() {
        let mut cal = SequentialCalibrator::new(1, "RGB", 1, 1, 255, 32);
        cal.record(&[0u8; 16], 4, 4);
        let map = cal.finish("t");
        assert_eq!(map.leds[0].conf, 0.0);
        assert_eq!(map.leds[0].u, 0.0);
    }

    /// 171st RGB fixture wraps to universe 2 channel 1 (no split) — must agree
    /// with the engine's packer convention.
    #[test]
    fn address_wraps_without_splitting() {
        let cal = SequentialCalibrator::new(200, "RGB", 1, 1, 255, 32);
        assert_eq!(cal.address_of(0), (1, 1));
        assert_eq!(cal.address_of(169), (1, 508));
        assert_eq!(cal.address_of(170), (2, 1));
    }
}
