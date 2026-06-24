//! Cue hierarchy — the heart of QPlayer's domain model.
//!
//! All cue types derive from a common base (via `CueBase` fields). JSON serialization
//! uses an internal tag (`$type`) to match C# `PolymorphicTypeResolver` output exactly.

use crate::{CanvasFit, SerializedColour, Timespan};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Shared fields present in every cue type.
///
/// In C# these live in the `Cue` base record; in Rust they are duplicated in each
/// enum variant (matching the flattened JSON shape) and accessible via `Cue::base()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CueBase {
    pub qid: Decimal,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent: Option<Decimal>,
    #[serde(default)]
    pub colour: SerializedColour,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub trigger: TriggerMode,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub delay: Timespan,
    #[serde(default)]
    pub loop_mode: LoopMode,
    #[serde(default = "default_one")]
    pub loop_count: i32,
    #[serde(default)]
    pub remote_node: String,
    #[serde(default)]
    pub triggers: CueTriggers,
}

impl Default for CueBase {
    fn default() -> Self {
        Self {
            qid: Decimal::ZERO,
            parent: None,
            colour: SerializedColour::BLACK,
            name: String::new(),
            description: String::new(),
            trigger: TriggerMode::Go,
            enabled: true,
            delay: Timespan::ZERO,
            loop_mode: LoopMode::OneShot,
            loop_count: 1,
            remote_node: String::new(),
            triggers: CueTriggers::default(),
        }
    }
}

/// Optional alternate firing methods for a cue (Triggers tab).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CueTriggers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<HotkeyTrigger>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi: Option<MidiTrigger>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_clock: Option<WallClockTrigger>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timecode: Option<TimecodeTrigger>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotkeyTrigger {
    pub key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MidiTriggerKind {
    NoteOn,
    NoteOff,
    CC,
}

impl Default for MidiTriggerKind {
    fn default() -> Self {
        MidiTriggerKind::NoteOn
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MidiTrigger {
    pub channel: u8,
    #[serde(default)]
    pub kind: MidiTriggerKind,
    pub note_or_cc: u8,
    #[serde(default)]
    pub velocity_min: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClockMode {
    TwelveHour,
    #[serde(rename = "TwentyFourHour")]
    TwentyFourHour,
}

impl Default for ClockMode {
    fn default() -> Self {
        ClockMode::TwentyFourHour
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatMode {
    Daily,
    Once,
}

impl Default for RepeatMode {
    fn default() -> Self {
        RepeatMode::Daily
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WallClockTrigger {
    pub time: String,
    #[serde(default)]
    pub mode: ClockMode,
    #[serde(default)]
    pub repeat: RepeatMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimecodeTrigger {
    pub time: Timespan,
}

/// Polymorphic cue enum with `$type` discriminator.
///
/// Serializes to match C# output:
/// ```json
/// { "$type": "SoundCue", "qid": 1, "name": "Intro", "path": "intro.wav" }
/// ```
/// Per-cue output routing.
///
/// `crosspoints` empty → lightweight stereo: the cue's stereo signal goes to one
/// output pair (0 = outs 1-2, 1 = 3-4, ...) at `send`. Non-empty → full
/// input×output matrix: each [`Crosspoint`] routes one source channel to one
/// output channel at a gain (handles multichannel sources such as 5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioRouting {
    /// Output pair index for the lightweight route (used when `crosspoints` empty).
    #[serde(default)]
    pub out_pair: u8,
    /// Send level (linear gain) for the lightweight pair route. Default 1.0.
    #[serde(default = "unity_send")]
    pub send: f32,
    /// Crosspoint matrix. When non-empty, overrides the pair route.
    #[serde(default)]
    pub crosspoints: Vec<Crosspoint>,
}

/// One source-channel → output-channel routing at a linear gain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Crosspoint {
    #[serde(default)]
    pub in_ch: u8,
    #[serde(default)]
    pub out_ch: u8,
    #[serde(default = "unity_send")]
    pub gain: f32,
}

fn unity_send() -> f32 {
    1.0
}

impl Default for AudioRouting {
    fn default() -> Self {
        Self { out_pair: 0, send: 1.0, crosspoints: Vec::new() }
    }
}

impl Default for Crosspoint {
    fn default() -> Self {
        Self { in_ch: 0, out_ch: 0, gain: 1.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum Cue {
    #[serde(rename = "GroupCue")]
    Group {
        #[serde(flatten)]
        base: CueBase,
    },

    #[serde(rename = "DummyCue")]
    Dummy {
        #[serde(flatten)]
        base: CueBase,
    },

    #[serde(rename = "SoundCue")]
    Sound {
        #[serde(flatten)]
        base: CueBase,
        #[serde(default)]
        path: String,
        #[serde(default)]
        start_time: Timespan,
        #[serde(default)]
        duration: Timespan,
        #[serde(default)]
        volume: f32,
        #[serde(default)]
        pan: f32,
        #[serde(default)]
        fade_in: f32,
        #[serde(default)]
        fade_out: f32,
        #[serde(default)]
        fade_type: FadeType,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        eq: Option<crate::EQSettings>,
        #[serde(default)]
        routing: AudioRouting,
    },

    #[serde(rename = "TimeCodeCue")]
    TimeCode {
        #[serde(flatten)]
        base: CueBase,
        #[serde(default)]
        start_time: Timespan,
        #[serde(default)]
        duration: Timespan,
    },

    #[serde(rename = "StopCue")]
    Stop {
        #[serde(flatten)]
        base: CueBase,
        #[serde(default)]
        stop_qid: Decimal,
        #[serde(default)]
        stop_mode: StopMode,
        #[serde(default)]
        fade_out_time: f32,
        #[serde(default)]
        fade_type: FadeType,
    },

    #[serde(rename = "VolumeCue")]
    Volume {
        #[serde(flatten)]
        base: CueBase,
        #[serde(default)]
        sound_qid: Decimal,
        #[serde(default)]
        fade_time: f32,
        #[serde(default)]
        volume: f32,
        #[serde(default)]
        fade_type: FadeType,
    },

    #[serde(rename = "VideoCue")]
    Video {
        #[serde(flatten)]
        base: CueBase,
        #[serde(default)]
        path: String,
        #[serde(default)]
        start_time: Timespan,
        #[serde(default)]
        duration: Timespan,
        #[serde(default)]
        volume: f32,
        #[serde(default)]
        pan: f32,
        #[serde(default)]
        fade_in: f32,
        #[serde(default)]
        fade_out: f32,
        #[serde(default)]
        fade_type: FadeType,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        eq: Option<crate::EQSettings>,
        #[serde(default)]
        routing: AudioRouting,
    },

    #[serde(rename = "OSCCue")]
    Osc {
        #[serde(flatten)]
        base: CueBase,
        #[serde(default)]
        command: String,
    },

    #[serde(rename = "TextCue")]
    Text {
        #[serde(flatten)]
        base: CueBase,
        #[serde(default)]
        text: String,
        #[serde(default = "default_font_size")]
        font_size: f32,
        #[serde(default)]
        font_colour: SerializedColour,
        #[serde(default)]
        fit: CanvasFit,
    },

    #[serde(rename = "ImageCue")]
    Image {
        #[serde(flatten)]
        base: CueBase,
        #[serde(default)]
        path: String,
        #[serde(default)]
        fit: CanvasFit,
    },

    #[serde(rename = "GotoCue")]
    Goto {
        #[serde(flatten)]
        base: CueBase,
        #[serde(default)]
        target_qid: Decimal,
    },
}

// Convenience type aliases for pattern matching ergonomics.
pub type GroupCue = Cue;
pub type DummyCue = Cue;
pub type SoundCue = Cue;
pub type TimeCodeCue = Cue;
pub type StopCue = Cue;
pub type VolumeCue = Cue;
pub type VideoCue = Cue;
pub type OscCue = Cue;
pub type TextCue = Cue;
pub type ImageCue = Cue;
pub type GotoCue = Cue;

impl Cue {
    /// Access the shared base fields of any cue variant.
    pub fn base(&self) -> &CueBase {
        match self {
            Cue::Group { base, .. } => base,
            Cue::Dummy { base, .. } => base,
            Cue::Sound { base, .. } => base,
            Cue::TimeCode { base, .. } => base,
            Cue::Stop { base, .. } => base,
            Cue::Volume { base, .. } => base,
            Cue::Video { base, .. } => base,
            Cue::Osc { base, .. } => base,
            Cue::Text { base, .. } => base,
            Cue::Image { base, .. } => base,
            Cue::Goto { base, .. } => base,
        }
    }

    /// Mutable access to base fields.
    pub fn base_mut(&mut self) -> &mut CueBase {
        match self {
            Cue::Group { base, .. } => base,
            Cue::Dummy { base, .. } => base,
            Cue::Sound { base, .. } => base,
            Cue::TimeCode { base, .. } => base,
            Cue::Stop { base, .. } => base,
            Cue::Volume { base, .. } => base,
            Cue::Video { base, .. } => base,
            Cue::Osc { base, .. } => base,
            Cue::Text { base, .. } => base,
            Cue::Image { base, .. } => base,
            Cue::Goto { base, .. } => base,
        }
    }

    /// The cue number (QID).
    pub fn qid(&self) -> Decimal {
        self.base().qid
    }

    /// Short human-readable name.
    pub fn name(&self) -> &str {
        &self.base().name
    }

    /// Whether this cue is enabled.
    pub fn enabled(&self) -> bool {
        self.base().enabled
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LoopMode {
    #[default]
    OneShot,
    Looped,
    LoopedInfinite,
    HoldLast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StopMode {
    #[default]
    Immediate,
    LoopEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TriggerMode {
    /// Execute immediately when triggered.
    #[default]
    Go,
    /// Execute concurrently with the previous cue.
    WithLast,
    /// Execute after the previous cue completes.
    AfterLast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FadeType {
    Linear,
    #[default]
    SCurve,
    Square,
    InverseSquare,
}

fn default_true() -> bool {
    true
}

fn default_one() -> i32 {
    1
}

fn default_font_size() -> f32 {
    48.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_sound_cue_serde() {
        let cue = Cue::Sound {
            base: CueBase {
                qid: Decimal::from(1),
                name: "Intro Music".into(),
                ..Default::default()
            },
            path: "audio/intro.wav".into(),
            start_time: Timespan::ZERO,
            duration: Timespan::from_secs_f64(120.0),
            volume: 0.0,
            pan: 0.0,
            fade_in: 2.0,
            fade_out: 3.0,
            fade_type: FadeType::SCurve,
            eq: None,
            routing: AudioRouting::default(),
        };

        let json = serde_json::to_string_pretty(&cue).unwrap();
        println!("{}", json);

        let de: Cue = serde_json::from_str(&json).unwrap();
        assert_eq!(cue, de);
    }

    #[test]
    fn test_cue_base_access() {
        let cue = Cue::Dummy {
            base: CueBase {
                qid: Decimal::from(5),
                name: "Note".into(),
                ..Default::default()
            },
        };
        assert_eq!(cue.qid(), Decimal::from(5));
        assert_eq!(cue.name(), "Note");
    }

    #[test]
    fn test_polymorphic_tag() {
        // Verify the JSON has the $type discriminator at the top level.
        let cue = Cue::Group {
            base: CueBase::default(),
        };
        let json = serde_json::to_value(&cue).unwrap();
        assert_eq!(json["$type"], "GroupCue");
    }

    #[test]
    fn test_load_csharp_style_json() {
        // Simulate a C#-generated show file cue.
        let json = json!({
            "$type": "SoundCue",
            "qid": 1.0,
            "parent": null,
            "colour": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 },
            "name": "Test Sound",
            "description": "",
            "trigger": "Go",
            "enabled": true,
            "delay": "00:00:00",
            "loopMode": "OneShot",
            "loopCount": 1,
            "remoteNode": "",
            "path": "test.wav",
            "startTime": "00:00:00",
            "duration": "00:00:00",
            "volume": 0.0,
            "pan": 0.0,
            "fadeIn": 0.0,
            "fadeOut": 0.0,
            "fadeType": "SCurve"
        });

        let cue: Cue = serde_json::from_value(json).expect("should parse C#-style cue");
        match cue {
            Cue::Sound { base, path, .. } => {
                assert_eq!(base.qid, Decimal::from(1));
                assert_eq!(base.name, "Test Sound");
                assert_eq!(path, "test.wav");
            }
            other => panic!("expected SoundCue, got {:?}", other),
        }
    }

    #[test]
    fn test_osc_cue_serde() {
        let cue = Cue::Osc {
            base: CueBase {
                qid: Decimal::from(7),
                name: "OSC Go".into(),
                ..Default::default()
            },
            command: "/qplayer/go,5".into(),
        };
        let json = serde_json::to_string(&cue).unwrap();
        let de: Cue = serde_json::from_str(&json).unwrap();
        assert_eq!(cue, de);
        let val = serde_json::to_value(&cue).unwrap();
        assert_eq!(val["$type"], "OSCCue");
        assert_eq!(val["command"], "/qplayer/go,5");
    }

    #[test]
    fn test_video_cue_serde() {
        let cue = Cue::Video {
            base: CueBase {
                qid: Decimal::from(3),
                name: "Intro Video".into(),
                ..Default::default()
            },
            path: "video/intro.mp4".into(),
            start_time: Timespan::ZERO,
            duration: Timespan::from_secs_f64(60.0),
            volume: -6.0,
            pan: 0.0,
            fade_in: 1.0,
            fade_out: 1.0,
            fade_type: FadeType::SCurve,
            eq: None,
            routing: AudioRouting::default(),
        };
        let json = serde_json::to_string(&cue).unwrap();
        let de: Cue = serde_json::from_str(&json).unwrap();
        assert_eq!(cue, de);
        // Verify the tag
        let val = serde_json::to_value(&cue).unwrap();
        assert_eq!(val["$type"], "VideoCue");
    }

    #[test]
    fn test_text_cue_serde() {
        let cue = Cue::Text {
            base: CueBase {
                qid: Decimal::from(10),
                name: "Title".into(),
                ..Default::default()
            },
            text: "Hello QPlayer".into(),
            font_size: 64.0,
            font_colour: SerializedColour::WHITE,
            fit: CanvasFit::Fit,
        };
        let json = serde_json::to_string(&cue).unwrap();
        let de: Cue = serde_json::from_str(&json).unwrap();
        assert_eq!(cue, de);
        let val = serde_json::to_value(&cue).unwrap();
        assert_eq!(val["$type"], "TextCue");
    }

    #[test]
    fn test_image_cue_serde() {
        let cue = Cue::Image {
            base: CueBase {
                qid: Decimal::from(11),
                name: "Logo".into(),
                ..Default::default()
            },
            path: "logo.png".into(),
            fit: CanvasFit::Fill,
        };
        let json = serde_json::to_string(&cue).unwrap();
        let de: Cue = serde_json::from_str(&json).unwrap();
        assert_eq!(cue, de);
        let val = serde_json::to_value(&cue).unwrap();
        assert_eq!(val["$type"], "ImageCue");
    }

    #[test]
    fn test_goto_cue_serde() {
        let cue = Cue::Goto {
            base: CueBase {
                qid: Decimal::from(12),
                name: "Jump".into(),
                ..Default::default()
            },
            target_qid: Decimal::from(5),
        };
        let json = serde_json::to_string(&cue).unwrap();
        let de: Cue = serde_json::from_str(&json).unwrap();
        assert_eq!(cue, de);
        let val = serde_json::to_value(&cue).unwrap();
        assert_eq!(val["$type"], "GotoCue");
    }

    #[test]
    fn test_triggers_default_empty() {
        let cue = Cue::Dummy {
            base: CueBase::default(),
        };
        let json = serde_json::to_string(&cue).unwrap();
        let de: Cue = serde_json::from_str(&json).unwrap();
        assert_eq!(cue.base().triggers, de.base().triggers);
        assert!(de.base().triggers.hotkey.is_none());
    }

    #[test]
    fn test_triggers_roundtrip() {
        let triggers = CueTriggers {
            hotkey: Some(HotkeyTrigger { key: "Space".into() }),
            midi: Some(MidiTrigger {
                channel: 1,
                kind: MidiTriggerKind::NoteOn,
                note_or_cc: 60,
                velocity_min: 1,
            }),
            wall_clock: Some(WallClockTrigger {
                time: "14:30:00".into(),
                mode: ClockMode::TwentyFourHour,
                repeat: RepeatMode::Daily,
            }),
            timecode: Some(TimecodeTrigger {
                time: Timespan::from_secs_f64(10.0),
            }),
        };
        let cue = Cue::Dummy {
            base: CueBase {
                qid: Decimal::ONE,
                triggers,
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&cue).unwrap();
        let de: Cue = serde_json::from_str(&json).unwrap();
        assert_eq!(cue, de);
    }
}
