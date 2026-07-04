//! 4-band semi-parametric EQ settings.
//!
//! Matches C# `EQSettings`, `EQBand`, `EQBandShape`, `EQFilter`, `EQFilterOrder`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EQSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub band1: EQBand,
    #[serde(default)]
    pub band2: EQBand,
    #[serde(default)]
    pub band3: EQBand,
    #[serde(default)]
    pub band4: EQBand,
    #[serde(default)]
    pub hpf: EQFilter,
    #[serde(default)]
    pub lpf: EQFilter,
}

impl Default for EQSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            band1: EQBand::default(),
            band2: EQBand::default(),
            band3: EQBand::default(),
            band4: EQBand::default(),
            hpf: EQFilter {
                frequency: 20.0,
                order: EQFilterOrder::default(),
            },
            lpf: EQFilter {
                frequency: 20000.0,
                order: EQFilterOrder::default(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct EQBand {
    #[serde(default)]
    pub freq: f32,
    #[serde(default)]
    pub gain: f32,
    #[serde(default = "default_q")]
    pub q: f32,
    #[serde(default)]
    pub shape: EQBandShape,
}

fn default_q() -> f32 {
    0.7
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EQBandShape {
    #[default]
    Bell,
    HighShelf,
    LowShelf,
    Notch,
    LowPass,
    HighPass,
    AllPass,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct EQFilter {
    #[serde(default)]
    pub frequency: f32,
    #[serde(default)]
    pub order: EQFilterOrder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EQFilterOrder {
    #[default]
    Disabled,
    #[serde(rename = "_12dBOct")]
    _12dBOct,
    #[serde(rename = "_24dBOct")]
    _24dBOct,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eq_band_default_q() {
        let band: EQBand = serde_json::from_str("{}").unwrap();
        assert_eq!(band.q, 0.7);
    }

    #[test]
    fn test_eq_settings_serde() {
        let eq = EQSettings {
            enabled: true,
            band1: EQBand {
                freq: 100.0,
                gain: 3.0,
                q: 1.0,
                shape: EQBandShape::Bell,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&eq).unwrap();
        let de: EQSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(eq, de);
    }
}
