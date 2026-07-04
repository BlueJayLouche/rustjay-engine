//! Lighting patch and output configuration.
//!
//! CuePool reuses `rustjay-lighting`'s fixture types directly (profiles,
//! channel roles, looks) — no parallel type layer. This module adds only what
//! a show file needs on top: which fixtures exist (the patch) and where DMX
//! goes (protocol/destination).

use serde::{Deserialize, Serialize};

pub use rustjay_lighting::{
    builtin_profiles, render_look, Axis, ChannelRole, Corner, FixtureLook, FixtureProfile,
    ProfileId, ScanOrder, SegmentColor, WhiteMode,
};

/// Stable identity of a patched fixture, referenced by lighting cues.
pub type FixtureId = u32;

/// One fixture in the patch: a profile at a universe/address.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatchedFixture {
    pub id: FixtureId,
    #[serde(default)]
    pub name: String,
    pub profile_id: ProfileId,
    #[serde(default = "one_u16")]
    pub universe: u16,
    /// 1-based DMX start address.
    #[serde(default = "one_u16")]
    pub address: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LightingProtocol {
    #[default]
    Sacn,
    ArtNet,
}

/// What a pixel-map segment samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SegmentSource {
    /// The projection canvas (what the output windows show).
    #[default]
    Canvas,
    /// The dedicated pixel-map texture, fed by PixelMap cues — LED content
    /// independent of the projector picture.
    PixelMap,
}

/// One pixel-mapped region of a source texture driving a grid of fixtures
/// (vjarda-style). Segments stream live while the source has content; their
/// channels override lighting-cue looks on the same addresses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PixelMapSegment {
    pub id: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub source: SegmentSource,
    /// Source region, normalized `[x, y, w, h]` in 0..1.
    #[serde(default = "full_region")]
    pub region: [f32; 4],
    #[serde(default = "one_u32")]
    pub cols: u32,
    #[serde(default = "one_u32")]
    pub rows: u32,
    pub profile_id: ProfileId,
    #[serde(default = "one_u16")]
    pub universe: u16,
    /// 1-based DMX start address.
    #[serde(default = "one_u16")]
    pub address: u16,
    #[serde(default)]
    pub order: ScanOrder,
    /// Colour adjustments. Note: `SegmentColor::default()` white mode is
    /// MinSubtract (RGBW-oriented); [`Self::new`] overrides it to Off, which is
    /// correct for plain RGB — MinSubtract would blank white content.
    #[serde(default = "segment_color_default")]
    pub color: SegmentColor,
    /// Output gamma: display-referred canvas → LED-linear intensity.
    #[serde(default = "default_gamma")]
    pub gamma: f32,
}

/// Project-level lighting configuration, saved in the show file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub protocol: LightingProtocol,
    /// Unicast destination IP; empty = sACN multicast / Art-Net broadcast.
    #[serde(default)]
    pub dest_ip: String,
    /// DMX refresh rate.
    #[serde(default = "default_fps")]
    pub fps: f32,
    #[serde(default)]
    pub fixtures: Vec<PatchedFixture>,
    /// User-defined profiles; builtins are always available in addition.
    #[serde(default)]
    pub profiles: Vec<FixtureProfile>,
    /// Pixel-map segments sampling the video canvas.
    #[serde(default)]
    pub segments: Vec<PixelMapSegment>,
}

impl Default for LightingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            protocol: LightingProtocol::default(),
            dest_ip: String::new(),
            fps: default_fps(),
            fixtures: Vec::new(),
            profiles: Vec::new(),
            segments: Vec::new(),
        }
    }
}

impl LightingConfig {
    /// Resolve a profile id against user profiles first, then builtins.
    pub fn profile(&self, id: &str) -> Option<FixtureProfile> {
        self.profiles
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .or_else(|| builtin_profiles().into_iter().find(|p| p.id == id))
    }

    /// All profiles selectable in the UI: user-defined then builtins.
    pub fn all_profiles(&self) -> Vec<FixtureProfile> {
        let mut out = self.profiles.clone();
        out.extend(builtin_profiles());
        out
    }

    pub fn next_fixture_id(&self) -> FixtureId {
        self.fixtures.iter().map(|f| f.id).max().map_or(1, |m| m + 1)
    }

    pub fn next_segment_id(&self) -> u32 {
        self.segments.iter().map(|s| s.id).max().map_or(1, |m| m + 1)
    }

    /// Enabled segments with a resolvable profile — what the sampler must feed.
    pub fn active_segments(&self) -> impl Iterator<Item = &PixelMapSegment> {
        self.segments
            .iter()
            .filter(move |s| s.enabled && self.profile(&s.profile_id).is_some())
    }
}

fn one_u16() -> u16 {
    1
}

fn one_u32() -> u32 {
    1
}

fn default_fps() -> f32 {
    44.0
}

fn default_true() -> bool {
    true
}

fn default_gamma() -> f32 {
    2.2
}

fn full_region() -> [f32; 4] {
    [0.0, 0.0, 1.0, 1.0]
}

fn segment_color_default() -> SegmentColor {
    SegmentColor { white: WhiteMode::Off, ..Default::default() }
}

impl PixelMapSegment {
    /// A full-canvas RGB segment with sane defaults.
    pub fn new(id: u32) -> Self {
        Self {
            id,
            name: format!("Segment {id}"),
            enabled: true,
            source: SegmentSource::default(),
            region: full_region(),
            cols: 8,
            rows: 1,
            profile_id: "rgb".into(),
            universe: 1,
            address: 1,
            order: ScanOrder::default(),
            color: segment_color_default(),
            gamma: default_gamma(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrips_and_defaults() {
        let cfg = LightingConfig {
            enabled: true,
            fixtures: vec![PatchedFixture {
                id: 1,
                name: "Head L".into(),
                profile_id: "moving_head_16bit".into(),
                universe: 1,
                address: 1,
            }],
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let de: LightingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, de);

        // Old show files without the key must default cleanly.
        let empty: LightingConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, LightingConfig::default());
        assert_eq!(empty.fps, 44.0);
    }

    #[test]
    fn profile_lookup_prefers_user_over_builtin() {
        let mut cfg = LightingConfig::default();
        assert_eq!(cfg.profile("rgb").unwrap().name, "RGB");
        cfg.profiles.push(FixtureProfile {
            id: "rgb".into(),
            name: "My RGB".into(),
            channels: vec![ChannelRole::Red],
        });
        assert_eq!(cfg.profile("rgb").unwrap().name, "My RGB");
        assert!(cfg.profile("nope").is_none());
    }

    #[test]
    fn next_fixture_id_monotonic() {
        let mut cfg = LightingConfig::default();
        assert_eq!(cfg.next_fixture_id(), 1);
        cfg.fixtures.push(PatchedFixture {
            id: 7,
            name: String::new(),
            profile_id: "rgb".into(),
            universe: 1,
            address: 1,
        });
        assert_eq!(cfg.next_fixture_id(), 8);
    }
}
