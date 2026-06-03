//! Blend modes for channel compositing.
//!
//! The integer indices here are the contract with `composite.wgsl` — the shader
//! `switch`es on `CompositeParams.blend_mode`. Keep [`BlendMode::to_index`] and
//! the shader's `mode == Nu` branches in lockstep.

use serde::{Deserialize, Serialize};

/// How a channel is blended onto the running composite.
///
/// Ported from Varda's mixer. The ordinal of each variant is its shader index
/// (see [`BlendMode::to_index`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BlendMode {
    /// Alpha-over: source replaces destination weighted by source alpha × opacity.
    #[default]
    Normal,
    /// `clamp(src + dst)`.
    Add,
    /// `clamp(dst - src)`.
    Subtract,
    /// `src * dst`.
    Multiply,
    /// `1 - (1-src)(1-dst)`.
    Screen,
    /// Per-channel multiply/screen based on `dst < 0.5`.
    Overlay,
    /// Pegtop soft light.
    SoftLight,
    /// Per-channel multiply/screen based on `src < 0.5`.
    HardLight,
    /// `dst / (1 - src)`, clamped.
    ColorDodge,
    /// `1 - (1-dst) / src`, clamped.
    ColorBurn,
    /// `abs(src - dst)`.
    Difference,
    /// `src + dst - 2·src·dst`.
    Exclusion,
    /// `min(src, dst)`.
    Darken,
    /// `max(src, dst)`.
    Lighten,
    /// `max(src + dst - 1, 0)`.
    LinearBurn,
}

impl BlendMode {
    /// Shader uniform index. **Must** match the `mode == Nu` branches in
    /// `composite.wgsl`.
    pub fn to_index(self) -> u32 {
        match self {
            BlendMode::Normal => 0,
            BlendMode::Add => 1,
            BlendMode::Subtract => 2,
            BlendMode::Multiply => 3,
            BlendMode::Screen => 4,
            BlendMode::Overlay => 5,
            BlendMode::SoftLight => 6,
            BlendMode::HardLight => 7,
            BlendMode::ColorDodge => 8,
            BlendMode::ColorBurn => 9,
            BlendMode::Difference => 10,
            BlendMode::Exclusion => 11,
            BlendMode::Darken => 12,
            BlendMode::Lighten => 13,
            BlendMode::LinearBurn => 14,
        }
    }

    /// Short label for compact UI (4 chars).
    pub fn short_name(self) -> &'static str {
        match self {
            BlendMode::Normal => "Norm",
            BlendMode::Add => "Add",
            BlendMode::Subtract => "Sub",
            BlendMode::Multiply => "Mult",
            BlendMode::Screen => "Scrn",
            BlendMode::Overlay => "Ovly",
            BlendMode::SoftLight => "SftL",
            BlendMode::HardLight => "HrdL",
            BlendMode::ColorDodge => "CDge",
            BlendMode::ColorBurn => "CBrn",
            BlendMode::Difference => "Diff",
            BlendMode::Exclusion => "Excl",
            BlendMode::Darken => "Dark",
            BlendMode::Lighten => "Lite",
            BlendMode::LinearBurn => "LBrn",
        }
    }

    /// All variants in display / index order.
    pub fn all() -> &'static [BlendMode] {
        use BlendMode::*;
        &[
            Normal, Add, Subtract, Multiply, Screen, Overlay, SoftLight, HardLight,
            ColorDodge, ColorBurn, Difference, Exclusion, Darken, Lighten, LinearBurn,
        ]
    }

    /// Reverse lookup from shader uniform index.
    ///
    /// Returns `None` for out-of-range indices.
    pub fn from_index(index: u32) -> Option<Self> {
        match index {
            0 => Some(BlendMode::Normal),
            1 => Some(BlendMode::Add),
            2 => Some(BlendMode::Subtract),
            3 => Some(BlendMode::Multiply),
            4 => Some(BlendMode::Screen),
            5 => Some(BlendMode::Overlay),
            6 => Some(BlendMode::SoftLight),
            7 => Some(BlendMode::HardLight),
            8 => Some(BlendMode::ColorDodge),
            9 => Some(BlendMode::ColorBurn),
            10 => Some(BlendMode::Difference),
            11 => Some(BlendMode::Exclusion),
            12 => Some(BlendMode::Darken),
            13 => Some(BlendMode::Lighten),
            14 => Some(BlendMode::LinearBurn),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_fifteen_modes() {
        assert_eq!(BlendMode::all().len(), 15);
    }

    #[test]
    fn indices_are_contiguous_and_match_order() {
        // `all()` order must equal index order, and indices must be 0..15 with
        // no gaps or duplicates — that is the shader contract.
        for (i, mode) in BlendMode::all().iter().enumerate() {
            assert_eq!(mode.to_index(), i as u32, "{mode:?} index mismatch");
        }
    }

    #[test]
    fn default_is_normal() {
        assert_eq!(BlendMode::default(), BlendMode::Normal);
        assert_eq!(BlendMode::default().to_index(), 0);
    }

    #[test]
    fn from_index_roundtrips() {
        for &mode in BlendMode::all() {
            let idx = mode.to_index();
            assert_eq!(BlendMode::from_index(idx), Some(mode));
        }
        assert_eq!(BlendMode::from_index(99), None);
    }
}
