//! Show file migration — upgrade old `.qproj` files to current format.
//!
//! Ported from C# `ShowFileConverter`. Each upgrader is pure logic:
//! it takes a `serde_json::Value` (the raw parsed JSON) and mutates
//! a `ShowFile` in-place.

use crate::{SerializedColour, ShowFile, TriggerMode};
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
    if version < 8 {
        upgrade_v7_to_v8(show_file, raw);
    }
    if version < 9 {
        upgrade_v8_to_v9(show_file, raw);
    }
    if version < 10 {
        upgrade_v9_to_v10(show_file, raw);
    }

    show_file.file_format_version = crate::showfile::FILE_FORMAT_VERSION;
}

/// V2 -> V3: colour format changed from 0-255 byte to 0-1 float.
fn upgrade_v2_to_v3(show_file: &mut ShowFile, raw: &Value) {
    log::info!("Upgrading show file from V2 to V3...");

    // Upgrade showMetadata -> showSettings
    if let Some(meta) = raw.get("showMetadata")
        && let Ok(settings) = serde_json::from_value(meta.clone())
    {
        show_file.show_settings = settings;
    }

    // Upgrade cue colours from byte to float
    for (i, cue) in show_file.cues.iter_mut().enumerate() {
        if let Some(cues_arr) = raw.get("cues").and_then(|v| v.as_array())
            && let Some(cue_raw) = cues_arr.get(i)
            && let Some(colour_val) = cue_raw.get("colour")
        {
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

/// V3 -> V4: `halt` boolean replaced by `trigger` enum.
fn upgrade_v3_to_v4(show_file: &mut ShowFile, raw: &Value) {
    log::info!("Upgrading show file from V3 to V4...");

    for (i, cue) in show_file.cues.iter_mut().enumerate() {
        if let Some(cues_arr) = raw.get("cues").and_then(|v| v.as_array())
            && let Some(cue_raw) = cues_arr.get(i)
            && let Some(halt) = cue_raw.get("halt")
        {
            cue.base_mut().trigger = if halt.as_bool() == Some(true) {
                TriggerMode::Go
            } else {
                TriggerMode::WithLast
            };
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

/// V7 -> V8: added alternate cue triggers (hotkey, MIDI, wall-clock, timecode).
/// New fields use serde defaults, so this just logs the bump.
fn upgrade_v7_to_v8(_show_file: &mut ShowFile, _raw: &Value) {
    log::info!("Upgrading show file from V7 to V8...");
}

/// V8 -> V9: added MTC follow fields to Video cues (`follow_mtc`, `mtc_start`).
/// New fields use serde defaults, so this just logs the bump.
fn upgrade_v8_to_v9(_show_file: &mut ShowFile, _raw: &Value) {
    log::info!("Upgrading show file from V8 to V9...");
}

/// V9 -> V10: `osc_nic` named a network card, but selecting a card was never
/// what it did — its only effect was to derive an outbound destination, by
/// masking the address against a hardcoded /24. A blank field derived
/// `127.0.0.255`, so a project that never filled it in sent OSC nowhere but its
/// own machine (#213).
///
/// The destination is now stated directly in `osc_tx_host`. Carry a configured
/// card across as the broadcast address it used to produce — same /24 as the
/// code being replaced, so a working setup keeps working rather than silently
/// changing where it sends. A blank card needs nothing: loopback is already
/// `osc_tx_host`'s default, which preserves that case too.
fn upgrade_v9_to_v10(show_file: &mut ShowFile, _raw: &Value) {
    log::info!("Upgrading show file from V9 to V10...");

    let settings = &mut show_file.show_settings;
    let Ok(nic) = settings.osc_nic.parse::<std::net::Ipv4Addr>() else {
        return;
    };
    let o = nic.octets();
    let broadcast = std::net::Ipv4Addr::new(o[0], o[1], o[2], 255);
    settings.osc_tx_host = broadcast.to_string();
    log::info!("OSC destination migrated from NIC {nic} to {broadcast}");
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
                routing: crate::AudioRouting::default(),
            }],
            ..Default::default()
        };

        upgrade_v6_to_v7(&mut sf, &Value::Null);
        match &sf.cues[0] {
            Cue::Sound { volume, .. } => {
                assert!(
                    (volume - 0.0).abs() < 0.01,
                    "1.0 linear = 0 dB, got {}",
                    volume
                );
            }
            _ => panic!("expected SoundCue"),
        }
    }

    /// A V9 file that predates `osc_tx_host`.
    fn v9_show_file(osc_nic: &str) -> ShowFile {
        let mut sf = ShowFile {
            file_format_version: 9,
            ..Default::default()
        };
        sf.show_settings.osc_nic = osc_nic.into();
        sf
    }

    /// Someone with a working NIC must keep sending exactly where they were.
    /// The card is masked against the same /24 the replaced code used, so the
    /// destination is unchanged rather than merely plausible.
    #[test]
    fn v9_to_v10_carries_a_configured_nic_across_as_its_broadcast() {
        let mut sf = v9_show_file("10.0.1.42");

        upgrade_show_file(&mut sf, &Value::Null);

        assert_eq!(sf.show_settings.osc_tx_host, "10.0.1.255");
    }

    /// The #213 case: a blank card used to derive `127.0.0.255`, and now lands
    /// on the loopback default.
    ///
    /// This does **not** establish that the upgrader ran. `osc_tx_host` already
    /// defaults to loopback, so this passes with `upgrade_v9_to_v10` deleted
    /// outright — verified by mutation, not assumed.
    /// `v9_to_v10_carries_a_configured_nic_across_as_its_broadcast` is the test
    /// that pins the upgrader running at all.
    ///
    /// What this one pins is that the migration must not *guess* a network
    /// destination for a card nobody filled in. Guessing is what #213 proposed
    /// and what was deliberately declined: an upgrade must never take a machine
    /// that was quiet and start it broadcasting onto a live show network.
    #[test]
    fn v9_to_v10_leaves_a_blank_nic_on_loopback() {
        let mut sf = v9_show_file("");

        upgrade_show_file(&mut sf, &Value::Null);

        assert_eq!(sf.show_settings.osc_tx_host, "127.0.0.1");
    }

    /// A typo in the old field is not an address, and guessing what was meant
    /// would be worse than the documented default. Same caveat as
    /// `v9_to_v10_leaves_a_blank_nic_on_loopback`: this pins the absence of a
    /// guess, not the presence of the upgrader.
    #[test]
    fn v9_to_v10_leaves_an_unparseable_nic_on_loopback() {
        let mut sf = v9_show_file("the wired one");

        upgrade_show_file(&mut sf, &Value::Null);

        assert_eq!(sf.show_settings.osc_tx_host, "127.0.0.1");
    }

    /// The version gate is what stops the migration re-running: once a project
    /// is at V10 its destination is a deliberate choice, and a stale `osc_nic`
    /// left in the file must not overwrite it.
    #[test]
    fn a_v10_file_keeps_its_destination_despite_a_stale_nic() {
        let mut sf = ShowFile {
            file_format_version: 10,
            ..Default::default()
        };
        sf.show_settings.osc_nic = "10.0.1.42".into();
        sf.show_settings.osc_tx_host = "192.168.5.99".into();

        upgrade_show_file(&mut sf, &Value::Null);

        assert_eq!(sf.show_settings.osc_tx_host, "192.168.5.99");
    }
}
