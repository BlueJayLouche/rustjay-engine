//! Show file migration — upgrade old `.qproj` files to current format.
//!
//! Ported from C# `ShowFileConverter`. Each upgrader is pure logic:
//! it takes a `serde_json::Value` (the raw parsed JSON) and mutates
//! a `ShowFile` in-place.

use crate::{ShowFile, SerializedColour, TriggerMode};
use serde_json::Value;

/// Upgrade a show file from its on-disk version to `FILE_FORMAT_VERSION`.
pub fn upgrade_show_file(show_file: &mut ShowFile, raw: &Value) {
    let version = show_file.file_format_version;

    if version < 3 {
        upgrade_v2_to_v3(show_file, raw);
    }
    if version < 4 {
        upgrade_v3_to_v4(show_file, raw);
    }
    if version < 7 {
        upgrade_v6_to_v7(show_file, raw);
    }

    show_file.file_format_version = crate::showfile::FILE_FORMAT_VERSION;
}

/// V2 -> V3: colour format changed from 0-255 byte to 0-1 float.
fn upgrade_v2_to_v3(show_file: &mut ShowFile, raw: &Value) {
    log::info!("Upgrading show file from V2 to V3...");

    // Upgrade showMetadata -> showSettings
    if let Some(meta) = raw.get("showMetadata") {
        if let Ok(settings) = serde_json::from_value(meta.clone()) {
            show_file.show_settings = settings;
        }
    }

    // Upgrade cue colours from byte to float
    for (i, cue) in show_file.cues.iter_mut().enumerate() {
        if let Some(cues_arr) = raw.get("cues").and_then(|v| v.as_array()) {
            if let Some(cue_raw) = cues_arr.get(i) {
                if let Some(colour_val) = cue_raw.get("colour") {
                    let mut col = SerializedColour::BLACK;
                    if let Some(obj) = colour_val.as_object() {
                        if let Some(r) = obj.get("R").and_then(|v| v.as_u64()) {
                            col.r = (r as f32) / 255.0;
                        }
                        if let Some(g) = obj.get("G").and_then(|v| v.as_u64()) {
                            col.g = (g as f32) / 255.0;
                        }
                        if let Some(b) = obj.get("B").and_then(|v| v.as_u64()) {
                            col.b = (b as f32) / 255.0;
                        }
                        if let Some(a) = obj.get("A").and_then(|v| v.as_u64()) {
                            col.a = (a as f32) / 255.0;
                        }
                    }
                    cue.base_mut().colour = col;
                }
            }
        }
    }
}

/// V3 -> V4: `halt` boolean replaced by `trigger` enum.
fn upgrade_v3_to_v4(show_file: &mut ShowFile, raw: &Value) {
    log::info!("Upgrading show file from V3 to V4...");

    for (i, cue) in show_file.cues.iter_mut().enumerate() {
        if let Some(cues_arr) = raw.get("cues").and_then(|v| v.as_array()) {
            if let Some(cue_raw) = cues_arr.get(i) {
                if let Some(halt) = cue_raw.get("halt") {
                    cue.base_mut().trigger = if halt.as_bool() == Some(true) {
                        TriggerMode::Go
                    } else {
                        TriggerMode::WithLast
                    };
                }
            }
        }
    }
}

/// V6 -> V7: volume converted from linear to dB.
fn upgrade_v6_to_v7(show_file: &mut ShowFile, _raw: &Value) {
    log::info!("Upgrading show file from V6 to V7...");

    // MSC used to be enabled by default
    show_file.show_settings.enable_msc = true;

    for cue in &mut show_file.cues {
        match cue {
            crate::Cue::Sound { volume, .. } => {
                *volume = linear_to_db(*volume);
            }
            crate::Cue::Volume { volume, .. } => {
                *volume = linear_to_db(*volume);
            }
            crate::Cue::Video { volume, .. } => {
                *volume = linear_to_db(*volume);
            }
            _ => {}
        }
    }
}

#[inline]
fn linear_to_db(linear: f32) -> f32 {
    if linear <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * linear.log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cue, CueBase, SerializedColour};

    #[test]
    fn test_v2_to_v3_colour_upgrade() {
        let mut sf = ShowFile {
            cues: vec![Cue::Dummy {
                base: CueBase {
                    colour: SerializedColour::BLACK,
                    ..Default::default()
                },
            }],
            ..Default::default()
        };

        let raw = serde_json::json!({
            "cues": [{
                "colour": { "R": 128, "G": 64, "B": 32, "A": 255 }
            }]
        });

        upgrade_v2_to_v3(&mut sf, &raw);
        let col = sf.cues[0].base().colour;
        assert!((col.r - 0.50196).abs() < 0.01, "r = {}", col.r);
        assert!((col.g - 0.25098).abs() < 0.01, "g = {}", col.g);
        assert!((col.b - 0.12549).abs() < 0.01, "b = {}", col.b);
        assert!((col.a - 1.0).abs() < 0.01, "a = {}", col.a);
    }

    #[test]
    fn test_v3_to_v4_halt_upgrade() {
        let mut sf = ShowFile {
            cues: vec![Cue::Dummy {
                base: CueBase {
                    trigger: TriggerMode::Go,
                    ..Default::default()
                },
            }],
            ..Default::default()
        };

        let raw = serde_json::json!({
            "cues": [{ "halt": false }]
        });

        upgrade_v3_to_v4(&mut sf, &raw);
        assert_eq!(sf.cues[0].base().trigger, TriggerMode::WithLast);
    }

    #[test]
    fn test_v6_to_v7_volume_upgrade() {
        let mut sf = ShowFile {
            cues: vec![Cue::Sound {
                base: CueBase::default(),
                path: String::new(),
                start_time: crate::Timespan::ZERO,
                duration: crate::Timespan::ZERO,
                volume: 1.0,
                pan: 0.0,
                fade_in: 0.0,
                fade_out: 0.0,
                fade_type: crate::FadeType::SCurve,
                eq: None,
            }],
            ..Default::default()
        };

        upgrade_v6_to_v7(&mut sf, &Value::Null);
        match &sf.cues[0] {
            Cue::Sound { volume, .. } => {
                assert!((volume - 0.0).abs() < 0.01, "1.0 linear = 0 dB, got {}", volume);
            }
            _ => panic!("expected SoundCue"),
        }
    }
}
