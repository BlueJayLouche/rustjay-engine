//! Lighting patch and output configuration.
//!
//! CuePool reuses `rustjay-lighting`'s fixture types directly (profiles,
//! channel roles, looks) — no parallel type layer. This module adds only what
//! a show file needs on top: which fixtures exist (the patch) and where DMX
//! goes (protocol/destination).

use serde::{Deserialize, Serialize};

pub use rustjay_lighting::{
    Axis, ChannelRole, Corner, FixtureLook, FixtureProfile, ProfileId, ScanOrder, SegmentColor,
    WhiteMode, builtin_profiles, render_look,
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
    /// Unicast destination IP for this fixture's node (Art-Net/sACN); empty =
    /// project-level [`LightingConfig::dest_ip`].
    #[serde(default)]
    pub dest_ip: String,
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
    Canvas,
    /// The dedicated pixel-map texture, fed by PixelMap cues — LED content
    /// independent of the projector picture. Default: firing a PixelMap cue
    /// drives the LEDs with its own media, not whatever video is projecting.
    #[default]
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
    /// sACN-style merge priority of the fixture-look engine against recorded
    /// DMX shows (which carry their own per-cue priority).
    #[serde(default = "default_look_priority")]
    pub look_priority: u8,
}

fn default_look_priority() -> u8 {
    100
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
            look_priority: default_look_priority(),
        }
    }
}

impl PatchedFixture {
    /// Destination this fixture's DMX is sent to: own IP if set, else the
    /// project-level one. Empty = protocol default (multicast/broadcast).
    pub fn effective_dest<'a>(&'a self, global: &'a str) -> &'a str {
        let ip = self.dest_ip.trim();
        if ip.is_empty() { global.trim() } else { ip }
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
        self.fixtures
            .iter()
            .map(|f| f.id)
            .max()
            .map_or(1, |m| m + 1)
    }

    pub fn next_segment_id(&self) -> u32 {
        self.segments
            .iter()
            .map(|s| s.id)
            .max()
            .map_or(1, |m| m + 1)
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
    SegmentColor {
        white: WhiteMode::Off,
        ..Default::default()
    }
}

/// Patch sheet CSV for riggers/electricians: fixture summary rows with
/// per-channel detail underneath, plus pixel-map segment spans (one row per
/// universe a segment crosses), sorted by universe/address. The `Notes`
/// column flags address overlaps.
pub fn patch_sheet_csv(cfg: &LightingConfig) -> String {
    use rustjay_lighting::{PatchSpan, find_overlaps, segment_spans};

    let fixture_label = |f: &PatchedFixture| {
        if f.name.is_empty() {
            format!("Fixture {}", f.id)
        } else {
            f.name.clone()
        }
    };

    // Occupied spans (fixtures + all patched segments) for overlap detection.
    let mut spans: Vec<PatchSpan> = Vec::new();
    for f in &cfg.fixtures {
        let Some(p) = cfg.profile(&f.profile_id) else {
            continue;
        };
        spans.extend(segment_spans(
            fixture_label(f),
            p.name.clone(),
            f.universe,
            f.address,
            p.footprint(),
            1,
        ));
    }
    for s in &cfg.segments {
        let Some(p) = cfg.profile(&s.profile_id) else {
            continue;
        };
        spans.extend(segment_spans(
            s.name.clone(),
            p.name.clone(),
            s.universe,
            s.address,
            p.footprint(),
            (s.cols * s.rows) as usize,
        ));
    }
    let overlaps = find_overlaps(&spans);
    let overlap_note = |owner: &str, universe: u16, start: u16, end: u16| -> String {
        overlaps
            .iter()
            .filter(|o| {
                o.universe == universe
                    && start <= o.end
                    && o.start <= end
                    && (o.a.owner == owner || o.b.owner == owner)
            })
            .map(|o| {
                let other = if o.a.owner == owner {
                    &o.b.owner
                } else {
                    &o.a.owner
                };
                format!(
                    "OVERLAP with {} @ U{} {}-{}",
                    other, o.universe, o.start, o.end
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };

    let esc = |s: &str| -> String {
        if s.contains([',', '"', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };

    // (sort key, csv line) so fixtures/segments interleave by address.
    let mut rows: Vec<((u16, u16, u8), String)> = Vec::new();
    for f in &cfg.fixtures {
        let Some(p) = cfg.profile(&f.profile_id) else {
            continue;
        };
        let name = fixture_label(f);
        let end = f.address + p.footprint().max(1) as u16 - 1;
        let note = overlap_note(&name, f.universe, f.address, end);
        rows.push((
            (f.universe, f.address, 0),
            format!(
                "fixture,{},{},{},{},{},{},{},{}",
                esc(&name),
                esc(&p.name),
                f.universe,
                f.address,
                p.footprint(),
                end,
                esc(f.effective_dest(&cfg.dest_ip)),
                esc(&note)
            ),
        ));
        for (i, role) in p.channels.iter().enumerate() {
            let addr = f.address + i as u16;
            rows.push((
                (f.universe, f.address, 1 + i.min(254) as u8),
                format!(
                    "channel,{},{},{},{},1,{},,",
                    esc(&name),
                    esc(&role.describe()),
                    f.universe,
                    addr,
                    addr
                ),
            ));
        }
    }
    for s in &cfg.segments {
        let Some(p) = cfg.profile(&s.profile_id) else {
            continue;
        };
        let count = (s.cols * s.rows) as usize;
        for span in segment_spans(
            s.name.clone(),
            p.name.clone(),
            s.universe,
            s.address,
            p.footprint(),
            count,
        ) {
            let note = overlap_note(&s.name, span.universe, span.start, span.end);
            let layout = format!("{} × {} ({} ch/cell)", count, p.name, p.footprint());
            rows.push((
                (span.universe, span.start, 0),
                format!(
                    "segment,{},{},{},{},{},{},{},{}",
                    esc(&s.name),
                    esc(&layout),
                    span.universe,
                    span.start,
                    span.end - span.start + 1,
                    span.end,
                    esc(cfg.dest_ip.trim()),
                    esc(&note)
                ),
            ));
        }
    }
    rows.sort_by_key(|(k, _)| *k);

    let mut out = String::from("Kind,Name,Profile,Universe,Address,Channels,End,Dest IP,Notes\n");
    for (_, line) in rows {
        out.push_str(&line);
        out.push('\n');
    }
    out
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
                dest_ip: String::new(),
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
            dest_ip: String::new(),
        });
        assert_eq!(cfg.next_fixture_id(), 8);
    }
}

#[cfg(test)]
mod patch_sheet_tests {
    use super::*;

    #[test]
    fn patch_sheet_rows_and_overlaps() {
        let mut cfg = LightingConfig::default();
        cfg.fixtures.push(PatchedFixture {
            id: 1,
            name: "Spot L".into(),
            profile_id: "rgb".into(),
            universe: 1,
            address: 1,
            dest_ip: String::new(),
        });
        // Overlapping fixture: rgb footprint 3, so 1-3 and 2-4 collide.
        cfg.fixtures.push(PatchedFixture {
            id: 2,
            name: String::new(), // falls back to "Fixture 2"
            profile_id: "rgb".into(),
            universe: 1,
            address: 2,
            dest_ip: "10.0.0.5".into(),
        });
        let mut seg = PixelMapSegment::new(1);
        seg.universe = 2;
        seg.cols = 200; // 200 × 3ch = 600 ch → spans universes 2 and 3
        seg.rows = 1;
        cfg.segments.push(seg);

        let csv = patch_sheet_csv(&cfg);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(
            lines[0],
            "Kind,Name,Profile,Universe,Address,Channels,End,Dest IP,Notes"
        );
        // Fixture summary, then its channel rows, sorted by address.
        assert!(lines[1].starts_with("fixture,Spot L,"), "got {}", lines[1]);
        assert!(
            lines[2].starts_with("channel,Spot L,Red,1,1,"),
            "got {}",
            lines[2]
        );
        assert!(
            lines[1].contains("OVERLAP with Fixture 2"),
            "got {}",
            lines[1]
        );
        // Second fixture carries its per-fixture dest IP.
        let fx2 = lines
            .iter()
            .find(|l| l.starts_with("fixture,Fixture 2"))
            .unwrap();
        assert!(fx2.contains("10.0.0.5"));
        // Segment produces one row per universe crossed.
        let seg_rows: Vec<_> = lines.iter().filter(|l| l.starts_with("segment,")).collect();
        assert_eq!(seg_rows.len(), 2, "600ch tape spans two universes");
        assert!(seg_rows[0].contains(",2,1,"), "first span starts at U2 ch1");
    }
}
