//! `PointMap` — drive a recovered [`LedMap`](crate::LedMap) from rendered pixels.
//!
//! The engine's grid path (`rustjay_lighting::scan::demux_tile`) samples a
//! rectangular atlas tile. CV-mapped strips are not on a grid, so this samples
//! the canvas at each LED's freeform `(u,v)` and writes its channels straight to
//! the LED's own `universe`/`channel` (addresses may be non-contiguous, so this
//! does not use `pack_fixtures`).
//!
//! Playback loop (app side): read back the output texture to BGRA8, then
//! `point_map.sample(&bgra, w, h)` → `DmxSender::submit(frame)`.

use rustjay_lighting::{DmxFrame, DMX_UNIVERSE_SIZE};

use crate::format::LedMap;

/// A single color channel sourced from a sampled pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ch {
    R,
    G,
    B,
    /// White, derived as `min(r,g,b)`.
    W,
}

struct Point {
    u: f32,
    v: f32,
    universe: u16,
    /// 0-based start channel within the universe.
    ch0: usize,
    order: Vec<Ch>,
}

/// Samples a rendered frame into DMX channels per a [`LedMap`].
pub struct PointMap {
    points: Vec<Point>,
}

impl PointMap {
    /// Build from a loaded map. Unknown order letters are ignored; an LED with
    /// no usable channels is skipped.
    pub fn from_ledmap(map: &LedMap) -> Self {
        let points = map
            .leds
            .iter()
            .filter_map(|led| {
                let order: Vec<Ch> = led
                    .order
                    .chars()
                    .filter_map(|c| match c.to_ascii_uppercase() {
                        'R' => Some(Ch::R),
                        'G' => Some(Ch::G),
                        'B' => Some(Ch::B),
                        'W' => Some(Ch::W),
                        _ => None,
                    })
                    .collect();
                if order.is_empty() {
                    return None;
                }
                Some(Point {
                    u: led.u,
                    v: led.v,
                    universe: led.universe,
                    ch0: (led.channel.max(1) - 1) as usize,
                    order,
                })
            })
            .collect();
        Self { points }
    }

    /// Convenience: load a `ledmap.json` and build the sampler.
    pub fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        Ok(Self::from_ledmap(&LedMap::load(path)?))
    }

    /// Number of LEDs that will be driven.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// True if no LEDs are mapped.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// An all-zero [`DmxFrame`] covering every universe this map touches —
    /// submit on stop to turn the strip off.
    pub fn blackout(&self) -> DmxFrame {
        let mut frame = DmxFrame::new();
        for p in &self.points {
            frame.universe_mut(p.universe); // inserts a zeroed universe
        }
        frame
    }

    /// Sample a BGRA8 frame (`w*h*4` bytes, row-major) into a [`DmxFrame`].
    ///
    /// Each LED samples the nearest pixel to its `(u,v)`; channels are written at
    /// the LED's own address. Channels past a universe end are dropped (an LED's
    /// `channel` should leave room for its footprint).
    ///
    /// `// ponytail:` nearest-pixel + raw bytes — no gamma/gain/white-balance.
    /// Upgrade path: route each pixel through `rustjay_lighting::color_pipeline`
    /// with a per-segment `FixtureProfile` when output color needs correcting.
    pub fn sample(&self, bgra: &[u8], w: usize, h: usize) -> DmxFrame {
        let mut frame = DmxFrame::new();
        if w == 0 || h == 0 || bgra.len() < w * h * 4 {
            return frame;
        }
        for p in &self.points {
            let px = ((p.u.clamp(0.0, 1.0) * (w - 1) as f32).round() as usize).min(w - 1);
            let py = ((p.v.clamp(0.0, 1.0) * (h - 1) as f32).round() as usize).min(h - 1);
            let i = (py * w + px) * 4;
            let (b, g, r) = (bgra[i], bgra[i + 1], bgra[i + 2]);

            let buf = frame.universe_mut(p.universe);
            for (k, ch) in p.order.iter().enumerate() {
                let slot = p.ch0 + k;
                if slot >= DMX_UNIVERSE_SIZE {
                    break;
                }
                buf[slot] = match ch {
                    Ch::R => r,
                    Ch::G => g,
                    Ch::B => b,
                    Ch::W => r.min(g).min(b),
                };
            }
        }
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{Led, Source, Space, LEDMAP_VERSION};

    fn map_with(leds: Vec<Led>) -> LedMap {
        LedMap {
            version: LEDMAP_VERSION,
            space: Space::Canvas2d,
            source: Source { tool: "t".into(), captured: "t".into(), image_wh: [2, 2] },
            leds,
        }
    }

    /// GRB order reorders the sampled RGB; explicit address is honored.
    #[test]
    fn samples_and_reorders_to_address() {
        // 2x1 BGRA: left pixel = R, right pixel = G.
        let bgra = [
            0, 0, 255, 255, // (0,0) red   (B,G,R,A)
            0, 255, 0, 255, // (1,0) green
        ];
        let leds = vec![
            // LED 0 at far left (red), GRB at universe 1 ch 1.
            Led { i: 0, u: 0.0, v: 0.0, w: None, universe: 1, channel: 1, order: "GRB".into(), conf: 1.0 },
            // LED 1 at far right (green), GRB at universe 1 ch 7 (non-contiguous).
            Led { i: 1, u: 1.0, v: 0.0, w: None, universe: 1, channel: 7, order: "GRB".into(), conf: 1.0 },
        ];
        let pm = PointMap::from_ledmap(&map_with(leds));
        let frame = pm.sample(&bgra, 2, 1);
        let u = frame.get(1).expect("universe 1");
        // LED 0 red → GRB = [0, 255, 0] at ch 1..3.
        assert_eq!(&u[0..3], &[0, 255, 0]);
        // LED 1 green → GRB = [255, 0, 0] at ch 7..9 (0-based 6..8).
        assert_eq!(&u[6..9], &[255, 0, 0]);
    }

    /// RGBW derives W = min(r,g,b).
    #[test]
    fn rgbw_white_is_min_channel() {
        let bgra = [40, 80, 200, 255]; // B=40 G=80 R=200
        let leds = vec![Led {
            i: 0, u: 0.0, v: 0.0, w: None, universe: 2, channel: 1, order: "RGBW".into(), conf: 1.0,
        }];
        let pm = PointMap::from_ledmap(&map_with(leds));
        let frame = pm.sample(&bgra, 1, 1);
        let u = frame.get(2).unwrap();
        assert_eq!(&u[0..4], &[200, 80, 40, 40]); // R,G,B,W(min)
    }

    #[test]
    fn undersized_frame_is_empty() {
        let leds = vec![Led {
            i: 0, u: 0.0, v: 0.0, w: None, universe: 1, channel: 1, order: "RGB".into(), conf: 1.0,
        }];
        let pm = PointMap::from_ledmap(&map_with(leds));
        let frame = pm.sample(&[0, 0, 0], 4, 4); // too small
        assert!(frame.is_empty());
    }
}
