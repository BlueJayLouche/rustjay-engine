//! Fixture profiles and colour pipeline.
//!
//! A [`FixtureProfile`] describes the channel layout of one fixture (e.g. RGB,
//! RGBW, GRB). The [`color_pipeline`] turns one BGRA8 sampled pixel into the
//! fixture's channel bytes, applying output gamma, per-segment brightness/gain,
//! master dimmer, and RGBW white extraction.

use serde::{Deserialize, Serialize};

/// Identifies a fixture profile. Scene-level library uses human-readable ids
/// such as `"rgb"`, `"rgbw"`, etc.
pub type ProfileId = String;

/// One channel in a fixture's DMX footprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelRole {
    /// Red sampled from the pixel.
    Red,
    /// Green sampled from the pixel.
    Green,
    /// Blue sampled from the pixel.
    Blue,
    /// White LED channel, derived from `min(r,g,b)` via [`WhiteMode`].
    White,
    /// Amber channel (sampled or derived; M2 treats as a warm white approx.).
    Amber,
    /// UV / blacklight channel (sampled as blue-ish for now).
    Uv,
    /// Master dimmer, driven by [`SegmentColor::master_dimmer`].
    Dimmer,
    /// Constant byte (e.g. shutter open, mode select).
    Static(u8),
}

impl ChannelRole {
    /// Short single-letter label used in UI lists.
    pub fn label(&self) -> String {
        match self {
            ChannelRole::Red => "R".into(),
            ChannelRole::Green => "G".into(),
            ChannelRole::Blue => "B".into(),
            ChannelRole::White => "W".into(),
            ChannelRole::Amber => "A".into(),
            ChannelRole::Uv => "UV".into(),
            ChannelRole::Dimmer => "D".into(),
            ChannelRole::Static(v) => format!("S({})", v),
        }
    }
}

/// Description of one fixture type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureProfile {
    pub id: ProfileId,
    pub name: String,
    /// Channel layout in DMX order.
    pub channels: Vec<ChannelRole>,
}

impl FixtureProfile {
    /// Channels per fixture.
    pub fn footprint(&self) -> usize {
        self.channels.len()
    }
}

/// How to derive a `White` channel from an RGB pixel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum WhiteMode {
    /// No white channel.
    Off,
    /// Additive white: `w = min(r,g,b) * amount`. Brighter, less colour-accurate.
    Min { amount: f32 },
    /// Colour-accurate RGBW: extract `w = min(r,g,b) * amount` and subtract it
    /// from R/G/B. Equivalent to WLED's "Accurate" RGBW mode.
    MinSubtract { amount: f32 },
}

impl Default for WhiteMode {
    fn default() -> Self {
        Self::MinSubtract { amount: 1.0 }
    }
}

impl WhiteMode {
    /// UI label for the white-extraction mode.
    pub fn label(&self) -> &'static str {
        match self {
            WhiteMode::Off => "Off",
            WhiteMode::Min { .. } => "Min",
            WhiteMode::MinSubtract { .. } => "MinSubtract",
        }
    }
}

/// Per-segment colour adjustments applied after output gamma.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SegmentColor {
    /// 0..=1 global scale.
    #[serde(default = "one_f32")]
    pub brightness: f32,
    /// Per-channel R,G,B white-balance trim.
    #[serde(default = "default_gain")]
    pub gain: [f32; 3],
    /// 0..=1, drives [`ChannelRole::Dimmer`].
    #[serde(default = "one_f32")]
    pub master_dimmer: f32,
    /// White extraction for RGBW fixtures.
    #[serde(default)]
    pub white: WhiteMode,
}

impl Default for SegmentColor {
    fn default() -> Self {
        Self {
            brightness: 1.0,
            gain: [1.0; 3],
            master_dimmer: 1.0,
            white: WhiteMode::default(),
        }
    }
}

fn one_f32() -> f32 {
    1.0
}

fn default_gain() -> [f32; 3] {
    [1.0; 3]
}

/// Built-in fixture profiles shipped with the engine.
pub fn builtin_profiles() -> Vec<FixtureProfile> {
    vec![
        FixtureProfile {
            id: "rgb".into(),
            name: "RGB".into(),
            channels: vec![ChannelRole::Red, ChannelRole::Green, ChannelRole::Blue],
        },
        FixtureProfile {
            id: "grb".into(),
            name: "GRB".into(),
            channels: vec![ChannelRole::Green, ChannelRole::Red, ChannelRole::Blue],
        },
        FixtureProfile {
            id: "bgr".into(),
            name: "BGR".into(),
            channels: vec![ChannelRole::Blue, ChannelRole::Green, ChannelRole::Red],
        },
        FixtureProfile {
            id: "rgbw".into(),
            name: "RGBW".into(),
            channels: vec![
                ChannelRole::Red,
                ChannelRole::Green,
                ChannelRole::Blue,
                ChannelRole::White,
            ],
        },
        FixtureProfile {
            id: "rgb_dimmer".into(),
            name: "RGB + Dimmer".into(),
            channels: vec![
                ChannelRole::Red,
                ChannelRole::Green,
                ChannelRole::Blue,
                ChannelRole::Dimmer,
            ],
        },
    ]
}

/// Map one BGRA8 pixel to fixture channel bytes according to `profile`.
///
/// Pipeline:
/// 1. BGRA → RGB and normalise to 0..1.
/// 2. Apply output `gamma` (sRGB display → linear LED intensity).
/// 3. Apply `gain` (white balance) and `brightness`.
/// 4. Extract/subtract white for RGBW.
/// 5. Emit bytes in `ChannelRole` order.
pub fn color_pipeline(
    bgra: [u8; 4],
    gamma: f32,
    color: &SegmentColor,
    profile: &FixtureProfile,
) -> Vec<u8> {
    // 1. Reorder BGRA → RGB and normalise.
    let mut r = (bgra[2] as f32 / 255.0).powf(gamma);
    let mut g = (bgra[1] as f32 / 255.0).powf(gamma);
    let mut b = (bgra[0] as f32 / 255.0).powf(gamma);

    // 2. Gain and brightness.
    r *= color.gain[0] * color.brightness;
    g *= color.gain[1] * color.brightness;
    b *= color.gain[2] * color.brightness;

    // 3. White extraction.
    let w = match color.white {
        WhiteMode::Off => 0.0,
        WhiteMode::Min { amount } | WhiteMode::MinSubtract { amount } => {
            r.min(g).min(b) * amount
        }
    };
    if let WhiteMode::MinSubtract { .. } = color.white {
        r = (r - w).max(0.0);
        g = (g - w).max(0.0);
        b = (b - w).max(0.0);
    }

    // 4. Map to channel roles.
    let dimmer = (color.master_dimmer * 255.0).clamp(0.0, 255.0) as u8;
    let to_byte = |v: f32| (v * 255.0).clamp(0.0, 255.0) as u8;

    profile
        .channels
        .iter()
        .map(|role| match role {
            ChannelRole::Red => to_byte(r),
            ChannelRole::Green => to_byte(g),
            ChannelRole::Blue => to_byte(b),
            ChannelRole::White => to_byte(w),
            ChannelRole::Amber => to_byte((r + g) * 0.5), // warm white approx
            ChannelRole::Uv => to_byte(b * 0.8),
            ChannelRole::Dimmer => dimmer,
            ChannelRole::Static(v) => *v,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_profile_reorders_bgra() {
        let profile = FixtureProfile {
            id: "rgb".into(),
            name: "RGB".into(),
            channels: vec![ChannelRole::Red, ChannelRole::Green, ChannelRole::Blue],
        };
        // BGRA: blue=0, green=0, red=255, alpha=255
        let bytes = color_pipeline([0, 0, 255, 255], 2.2, &SegmentColor::default(), &profile);
        assert_eq!(bytes.len(), 3);
        // Only the red channel should be non-zero.
        assert!(bytes[0] > 200);
        assert_eq!(bytes[1], 0);
        assert_eq!(bytes[2], 0);
    }

    #[test]
    fn grb_profile_swaps_first_two_channels() {
        let profile = FixtureProfile {
            id: "grb".into(),
            name: "GRB".into(),
            channels: vec![ChannelRole::Green, ChannelRole::Red, ChannelRole::Blue],
        };
        let bytes = color_pipeline([0, 128, 64, 255], 2.2, &SegmentColor::default(), &profile);
        // BGRA: B=0, G=128, R=64 → linear G > R.
        assert!(bytes[0] > bytes[1]);
    }

    #[test]
    fn rgbw_minsubtract_extracts_white() {
        let profile = FixtureProfile {
            id: "rgbw".into(),
            name: "RGBW".into(),
            channels: vec![
                ChannelRole::Red,
                ChannelRole::Green,
                ChannelRole::Blue,
                ChannelRole::White,
            ],
        };
        // Neutral gray: B=G=R=128.
        let color = SegmentColor {
            white: WhiteMode::MinSubtract { amount: 1.0 },
            ..Default::default()
        };
        let bytes = color_pipeline([128, 128, 128, 255], 2.2, &color, &profile);
        // With MinSubtract, RGB should be near zero and white carries the luminance.
        assert!(bytes[0] < 5);
        assert!(bytes[1] < 5);
        assert!(bytes[2] < 5);
        assert!(bytes[3] > 20);
    }

    #[test]
    fn rgbw_min_keeps_rgb() {
        let profile = FixtureProfile {
            id: "rgbw".into(),
            name: "RGBW".into(),
            channels: vec![
                ChannelRole::Red,
                ChannelRole::Green,
                ChannelRole::Blue,
                ChannelRole::White,
            ],
        };
        let color = SegmentColor {
            white: WhiteMode::Min { amount: 1.0 },
            ..Default::default()
        };
        let bytes = color_pipeline([128, 128, 128, 255], 2.2, &color, &profile);
        // With Min (additive), RGB stay bright and white is added on top.
        assert!(bytes[0] > 20);
        assert!(bytes[1] > 20);
        assert!(bytes[2] > 20);
        assert!(bytes[3] > 20);
    }

    #[test]
    fn dimmer_role_scales_with_master_dimmer() {
        let profile = FixtureProfile {
            id: "rgb_dimmer".into(),
            name: "RGB + Dimmer".into(),
            channels: vec![
                ChannelRole::Red,
                ChannelRole::Green,
                ChannelRole::Blue,
                ChannelRole::Dimmer,
            ],
        };
        let color = SegmentColor {
            master_dimmer: 0.5,
            ..Default::default()
        };
        let bytes = color_pipeline([0, 0, 255, 255], 2.2, &color, &profile);
        assert_eq!(bytes[3], 127);
    }

    #[test]
    fn brightness_scales_all_sampled_channels() {
        let profile = FixtureProfile {
            id: "rgb".into(),
            name: "RGB".into(),
            channels: vec![ChannelRole::Red, ChannelRole::Green, ChannelRole::Blue],
        };
        let full = color_pipeline([0, 0, 255, 255], 2.2, &SegmentColor::default(), &profile);
        let half = color_pipeline(
            [0, 0, 255, 255],
            2.2,
            &SegmentColor {
                brightness: 0.5,
                ..Default::default()
            },
            &profile,
        );
        assert!(half[0] < full[0]);
    }

    #[test]
    fn gain_biases_channels() {
        let profile = FixtureProfile {
            id: "rgb".into(),
            name: "RGB".into(),
            channels: vec![ChannelRole::Red, ChannelRole::Green, ChannelRole::Blue],
        };
        // BGRA: B=0, G=128, R=128. Gain doubles green, keeps red.
        let color = SegmentColor {
            gain: [1.0, 2.0, 1.0],
            ..Default::default()
        };
        let bytes = color_pipeline([0, 128, 128, 255], 2.2, &color, &profile);
        // Green is boosted, so G > R.
        assert!(bytes[1] > bytes[0]);
    }
}
