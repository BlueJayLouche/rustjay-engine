//! `ledmap.json` v1 — the consumer-agnostic interchange format.
//!
//! Source of truth for a recovered LED layout. The engine's freeform point
//! sampler is the first consumer (samples the canvas at each LED's `(u,v)`); the
//! schema reserves `w` so a future 3D pass is additive, not a breaking change.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Current schema version written by this tool.
pub const LEDMAP_VERSION: u32 = 1;

/// Coordinate space the `(u,v[,w])` values live in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Space {
    /// 2D, normalized to the canvas/image. `u,v ∈ [0,1]`, `w` unused.
    #[serde(rename = "canvas-2d")]
    Canvas2d,
    /// 3D world coordinates (future multi-view pass). `u,v,w` populated.
    #[serde(rename = "world-3d")]
    World3d,
}

/// Provenance of a capture — enough to reproduce or debug a map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    /// Tool that produced the map, e.g. `"rustjay-ledmap"`.
    pub tool: String,
    /// RFC3339 capture time (free-form string; not parsed here).
    pub captured: String,
    /// Source image dimensions `[width, height]` the centroids came from.
    pub image_wh: [u32; 2],
}

/// One mapped LED: position + DMX patch address + color order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Led {
    /// LED index in wiring order (also its detection identity).
    pub i: u32,
    /// Horizontal position, `[0,1]` in [`Space::Canvas2d`].
    pub u: f32,
    /// Vertical position, `[0,1]` in [`Space::Canvas2d`].
    pub v: f32,
    /// Depth / z — only meaningful in [`Space::World3d`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<f32>,
    /// DMX universe this LED's channels live in.
    pub universe: u16,
    /// 1-based start channel within `universe`.
    pub channel: u16,
    /// Color byte order, e.g. `"GRB"` (ws281x) or `"RGBW"`.
    pub order: String,
    /// Detection confidence `[0,1]`; `0.0` marks an undetected LED to fix up.
    pub conf: f32,
}

/// A full recovered map — the contents of a `ledmap.json` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedMap {
    /// Schema version; see [`LEDMAP_VERSION`].
    pub version: u32,
    /// Space the coordinates are in.
    pub space: Space,
    /// Capture provenance.
    pub source: Source,
    /// Mapped LEDs, in wiring order.
    pub leds: Vec<Led>,
}

impl LedMap {
    /// Pretty-print to a JSON file.
    pub fn save(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load and parse a `ledmap.json` file.
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let map = LedMap {
            version: LEDMAP_VERSION,
            space: Space::Canvas2d,
            source: Source {
                tool: "rustjay-ledmap".into(),
                captured: "2026-06-22T12:00:00Z".into(),
                image_wh: [1920, 1080],
            },
            leds: vec![
                Led { i: 0, u: 0.12, v: 0.84, w: None, universe: 1, channel: 1, order: "GRB".into(), conf: 0.98 },
                Led { i: 1, u: 0.50, v: 0.50, w: None, universe: 1, channel: 4, order: "GRB".into(), conf: 0.0 },
            ],
        };
        let json = serde_json::to_string(&map).unwrap();
        // `w: None` must not appear in the wire form.
        assert!(!json.contains("\"w\""));
        assert!(json.contains("\"canvas-2d\""));

        let back: LedMap = serde_json::from_str(&json).unwrap();
        assert_eq!(back.leds.len(), 2);
        assert_eq!(back.leds[1].channel, 4);
        assert_eq!(back.leds[0].order, "GRB");
    }
}
