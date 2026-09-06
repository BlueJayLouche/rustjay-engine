//! Sources — ISF, video, image, camera, NDI, streams.
//!
//! Delegates to engine crates where possible:
//! - ISF      → `rustjay-isf`
//! - Camera   → `rustjay-io/input` (webcam)
//! - NDI      → `rustjay-io/ndi_runtime`
//! - Video decode / HAP / SRT / HLS / DASH / RTMP → coverage gaps;
//!   see `PARITY.md` Phase 2 / 9 / 10 probes.

mod camera_source;
mod image_source;
pub mod registry;

/// Playback speed for a clip, with tempo sync folded in.
///
/// Synced, the in/out span is stretched to last `BEAT_DIVISIONS[division]`
/// beats, so a clip loops on the bar however long it happens to be; `speed`
/// then trims on top of that. No tempo means no grid to lock to, so the clip
/// runs at `speed` alone rather than crawling.
pub fn clip_speed(speed: f32, sync: bool, division: usize, bpm: f32, span_seconds: f64) -> f32 {
    use rustjay_core::BEAT_DIVISIONS;
    if !sync || bpm <= 0.0 || span_seconds <= 0.0 {
        return speed;
    }
    let beats = BEAT_DIVISIONS[division.min(BEAT_DIVISIONS.len() - 1)] as f64;
    let target = beats * 60.0 / bpm as f64;
    speed * (span_seconds / target) as f32
}

/// How a clip responds to its `playing` parameter being driven.
///
/// The parameter is the same in every mode — a MIDI note, a key, an OSC
/// message. What differs is what happens on the edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerMode {
    /// Play/pause as told, from wherever the clip is. The `loop` setting stands.
    Latch,
    /// Plays while held. Each press starts at the in point, and letting go
    /// rewinds ready for the next one.
    Gate,
    /// One press plays the clip through once, from the in point, and stops.
    OneShot,
}

impl TriggerMode {
    /// From the `mode` parameter's index.
    pub fn from_index(i: i32) -> Self {
        match i {
            1 => Self::Gate,
            2 => Self::OneShot,
            _ => Self::Latch,
        }
    }

    /// Whether a press should rewind to the in point first.
    pub fn rewinds_on_press(self) -> bool {
        self != Self::Latch
    }

    /// Whether letting go should rewind, ready for the next press.
    pub fn rewinds_on_release(self) -> bool {
        self == Self::Gate
    }

    /// Whether letting go stops playback. A one-shot does not: PLAY is a
    /// momentary press, and the clip runs to the end on its own.
    pub fn stops_on_release(self) -> bool {
        self != Self::OneShot
    }

    /// The loop mode this trigger implies, or `None` to use the clip's own
    /// `loop` setting.
    pub fn loop_override(self) -> Option<i32> {
        match self {
            Self::Latch => None,
            // Held means playing, so a gate loops for as long as it is down.
            Self::Gate => Some(1),
            Self::OneShot => Some(0),
        }
    }
}

/// The two parameters every clip source adds for [`clip_speed`].
pub fn sync_parameters() -> Vec<rustjay_core::ParameterDescriptor> {
    use rustjay_core::{ParamCategory, ParameterDescriptor, lfo::BEAT_DIVISION_NAMES};
    let cat = ParamCategory::Custom("Playback".to_string());
    vec![
        ParameterDescriptor::enum_param(
            "mode",
            "Trigger",
            cat.clone(),
            vec!["Latch".into(), "Gate".into(), "One-shot".into()],
            0,
        ),
        ParameterDescriptor::bool("sync", "Sync", cat.clone(), false),
        ParameterDescriptor::enum_param(
            "div",
            "Division",
            cat,
            BEAT_DIVISION_NAMES.iter().map(|s| s.to_string()).collect(),
            // One whole note — a bar in 4/4.
            4,
        ),
    ]
}

#[cfg(test)]
mod clip_speed_tests {
    use super::clip_speed;

    /// A two-second span asked to fill a bar at 120 BPM (also two seconds)
    /// plays at 1×; the same span in half a bar plays at double speed.
    #[test]
    fn a_span_is_stretched_to_fill_the_division() {
        assert_eq!(clip_speed(1.0, true, 4, 120.0, 2.0), 1.0);
        assert_eq!(clip_speed(1.0, true, 3, 120.0, 2.0), 2.0);
    }

    #[test]
    fn speed_trims_the_synced_rate() {
        assert_eq!(clip_speed(0.5, true, 4, 120.0, 2.0), 0.5);
    }

    /// No tempo, or a clip with no span yet, must not divide by zero.
    #[test]
    fn nothing_to_lock_to_leaves_speed_alone() {
        assert_eq!(clip_speed(1.5, true, 4, 0.0, 2.0), 1.5);
        assert_eq!(clip_speed(1.5, true, 4, 120.0, 0.0), 1.5);
        assert_eq!(clip_speed(1.5, false, 4, 120.0, 2.0), 1.5);
    }
}
pub mod text_source;
mod solid_color_source;
mod watcher;

#[cfg(feature = "ffmpeg")]
mod ffmpeg_source;
#[cfg(feature = "ffmpeg")]
mod stream_source;
#[cfg(feature = "hap")]
mod hap_source;
#[cfg(feature = "ndi")]
mod ndi_source;
#[cfg(all(target_os = "windows", feature = "mixer"))]
mod spout_source;
#[cfg(all(target_os = "macos", feature = "mixer"))]
mod syphon_source;

pub use camera_source::CameraSource;
#[cfg(feature = "ffmpeg")]
pub use ffmpeg_source::FfmpegSource;
#[cfg(feature = "ffmpeg")]
pub use stream_source::StreamSource;
#[cfg(feature = "hap")]
pub use hap_source::HapSource;
pub use image_source::ImageSource;
#[cfg(feature = "ndi")]
pub use ndi_source::NdiSource;
pub use registry::{Registry, SourceEntry, SourceKind, classify_stream_url};
pub use solid_color_source::SolidColorSource;
pub use text_source::TextSource;
#[cfg(all(target_os = "windows", feature = "mixer"))]
pub use spout_source::SpoutSource;
#[cfg(all(target_os = "macos", feature = "mixer"))]
pub use syphon_source::SyphonSource;
pub use watcher::ShaderWatcher;

/// Test-only helpers. Not `#[cfg(test)]`: the integration tests are their own
/// crate and cannot see a unit-test-only module. It is a ZST with an empty
/// render, so it costs nothing to carry.
pub mod testing {
    use rustjay_core::{EffectInput, EffectInstance, EngineState, RenderCtx, RenderTarget};

    /// A source that needs no GPU — enough to stand a layer up in a test.
    pub struct StubSource;

    impl EffectInstance for StubSource {
        fn render_to(
            &mut self,
            _ctx: &mut RenderCtx<'_>,
            _inputs: &[EffectInput<'_>],
            _target: RenderTarget<'_>,
            _engine: &EngineState,
        ) {
        }
    }
}
