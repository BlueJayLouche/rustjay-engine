//! Cue hierarchy — the heart of QPlayer's domain model.
//!
//! All cue types derive from a common base (via `CueBase` fields). JSON serialization
//! uses an internal tag (`$type`) to match C# `PolymorphicTypeResolver` output exactly.

use crate::{SerializedColour, Timespan};
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
        }
    }
}

/// Polymorphic cue enum with `$type` discriminator.
///
/// Serializes to match C# output:
/// ```json
/// { "$type": "SoundCue", "qid": 1, "name": "Intro", "path": "intro.wav" }
/// ```
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
    },

    #[serde(rename = "OSCCue")]
    Osc {
        #[serde(flatten)]
        base: CueBase,
        #[serde(default)]
        command: String,
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
        };
        let json = serde_json::to_string(&cue).unwrap();
        let de: Cue = serde_json::from_str(&json).unwrap();
        assert_eq!(cue, de);
        // Verify the tag
        let val = serde_json::to_value(&cue).unwrap();
        assert_eq!(val["$type"], "VideoCue");
    }
}
