//! Portable RGBA colour representation.
//!
//! Matches C# `SerializedColour` exactly: four `f32` channels in [0, 1] range.

use serde::{Deserialize, Serialize};

/// A simple RGBA colour represented as 4 singles.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct SerializedColour {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl SerializedColour {
    pub const TRANSPARENT: Self = Self::new(0.0, 0.0, 0.0, 0.0);
    pub const BLACK: Self = Self::new(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Self = Self::new(1.0, 1.0, 1.0, 1.0);
    pub const RED: Self = Self::new(1.0, 0.0, 0.0, 1.0);
    pub const GREEN: Self = Self::new(0.0, 1.0, 0.0, 1.0);
    pub const BLUE: Self = Self::new(0.0, 0.0, 1.0, 1.0);

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Component-wise multiplication (used for colour tinting).
    #[inline]
    pub fn multiply(self, other: Self) -> Self {
        Self {
            r: self.r * other.r,
            g: self.g * other.g,
            b: self.b * other.b,
            a: self.a * other.a,
        }
    }

    /// Convert to 8-bit sRGB components.
    #[inline]
    pub fn to_u8_array(&self) -> [u8; 4] {
        [
            (self.r.clamp(0.0, 1.0) * 255.0) as u8,
            (self.g.clamp(0.0, 1.0) * 255.0) as u8,
            (self.b.clamp(0.0, 1.0) * 255.0) as u8,
            (self.a.clamp(0.0, 1.0) * 255.0) as u8,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consts() {
        assert_eq!(SerializedColour::BLACK, SerializedColour::new(0.0, 0.0, 0.0, 1.0));
        assert_eq!(SerializedColour::WHITE, SerializedColour::new(1.0, 1.0, 1.0, 1.0));
    }

    #[test]
    fn test_multiply() {
        let a = SerializedColour::new(1.0, 0.5, 0.0, 1.0);
        let b = SerializedColour::new(0.5, 0.5, 0.5, 1.0);
        let c = a.multiply(b);
        assert_eq!(c, SerializedColour::new(0.5, 0.25, 0.0, 1.0));
    }

    #[test]
    fn test_serde_roundtrip() {
        let col = SerializedColour::new(0.5, 0.25, 0.75, 1.0);
        let json = serde_json::to_string(&col).unwrap();
        let de: SerializedColour = serde_json::from_str(&json).unwrap();
        assert_eq!(col, de);
    }
}
