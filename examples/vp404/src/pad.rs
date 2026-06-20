//! An SP-404-style pad: wraps a [`Sample`] with Gate/Latch/One-Shot triggering and
//! frame-accurate playback within the sample's in/out points.
//!
//! Ported from rustjay-404 `src/sampler/pad.rs`, slimmed: the pad owns its `Sample`
//! directly (single-threaded render owner — no `Arc<Mutex>`), the to-file
//! `debug_log` is dropped, and the frame-advance math is extracted into the pure
//! [`advance`] fn so it's unit-testable without a GPU or a file.
#![allow(dead_code)] // model API — index/name/color/release/stop wired up in Phase 1b

use std::time::Duration;

use rustjay_core::lfo::{BEAT_DIVISIONS, BEAT_DIVISION_NAMES};
use serde::{Deserialize, Serialize};

use crate::sample::{ColorSpace, Sample};

/// SP-404 trigger behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TriggerMode {
    /// Play while held, stop on release.
    #[default]
    Gate,
    /// Toggle on/off with each trigger.
    Latch,
    /// Play once and stop at the out point.
    OneShot,
}

/// How a pad's playhead is advanced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlaybackMode {
    /// Classic SP-404: `speed` drives the frame counter.
    #[default]
    Free,
    /// Beat-locked: frame derived from engine `beat_phase` and a beat division.
    Synced,
}

impl PlaybackMode {
    pub fn to_index(self) -> usize {
        match self {
            PlaybackMode::Free => 0,
            PlaybackMode::Synced => 1,
        }
    }

    pub fn from_index(i: usize) -> Self {
        match i {
            1 => PlaybackMode::Synced,
            _ => PlaybackMode::Free,
        }
    }

    pub fn labels() -> &'static [&'static str] {
        &["Free", "Synced"]
    }
}

/// Advance a playhead one tick and report whether it's still playing.
///
/// Pure (no GPU / no I/O) so the loop/clamp logic is unit-testable. Mirrors
/// rustjay-404's `SamplePad::update` wrapping rules.
#[allow(clippy::too_many_arguments)]
pub fn advance(
    current: f32,
    speed: f32,
    fps: f32,
    dt: f32,
    in_f: f32,
    out_f: f32,
    mode: TriggerMode,
    loop_enabled: bool,
) -> (f32, bool) {
    let speed = speed.clamp(-5.0, 5.0);
    let mut frame = current + speed * fps * dt;

    if speed >= 0.0 {
        if frame >= out_f {
            // Forward: Latch/Gate/loop keep going; One-Shot stops at the end.
            if loop_enabled || mode == TriggerMode::Latch || mode == TriggerMode::Gate {
                if mode == TriggerMode::Gate && !loop_enabled {
                    frame = out_f; // hold at end while key held
                } else {
                    frame = in_f + (frame - out_f); // seamless wrap
                }
            } else {
                return (out_f, false);
            }
        }
    } else if frame <= in_f {
        // Reverse: Gate/loop keep going; otherwise stop at the start.
        if loop_enabled || mode == TriggerMode::Gate {
            if mode == TriggerMode::Gate && !loop_enabled {
                frame = in_f;
            } else {
                frame = out_f - (in_f - frame);
            }
        } else {
            return (in_f, false);
        }
    }
    (frame, true)
}

/// Compute the frame a synced pad should display for a given global beat count.
///
/// `beat` is total elapsed beats. `division` indexes [`BEAT_DIVISIONS`], which
/// stores the loop length in beats. The result is clamped to `[in_point, out_point]`.
pub fn synced_frame(beat: f32, division: usize, in_point: u32, out_point: u32) -> f32 {
    let beats_per_loop = BEAT_DIVISIONS[division.clamp(0, BEAT_DIVISIONS.len() - 1)];
    let loop_phase = (beat / beats_per_loop).fract().max(0.0);
    let range = out_point.saturating_sub(in_point) as f32;
    in_point as f32 + loop_phase * range
}

pub struct Pad {
    pub index: usize,
    pub name: String,
    pub color: [u8; 3],
    pub sample: Option<Sample>,

    pub trigger_mode: TriggerMode,
    pub loop_enabled: bool,
    pub playback_mode: PlaybackMode,
    /// Index into `BEAT_DIVISIONS` / `BEAT_DIVISION_NAMES` (used when `Synced`).
    pub beat_division: usize,
    /// Playback speed. In `Free` mode this is driven by the engine parameter
    /// `ch_pad<N>_speed` so MIDI/OSC/LFO can modulate it. In `Synced` mode the
    /// effective rate is derived from the engine tempo and this value is ignored.
    pub speed: f32,

    pub is_playing: bool,
    pub is_triggered: bool,
    pub current_frame: f32,
    /// 0..1, decays for UI trigger-flash animation.
    pub trigger_level: f32,
}

impl Pad {
    pub fn new(index: usize) -> Self {
        Self {
            index,
            name: format!("Pad {}", index + 1),
            color: Self::default_color(index),
            sample: None,
            trigger_mode: TriggerMode::default(),
            loop_enabled: false,
            playback_mode: PlaybackMode::default(),
            beat_division: 2, // 1/4 note
            speed: 1.0,
            is_playing: false,
            is_triggered: false,
            current_frame: 0.0,
            trigger_level: 0.0,
        }
    }

    fn default_color(index: usize) -> [u8; 3] {
        const COLORS: [[u8; 3]; 8] = [
            [255, 100, 100],
            [255, 180, 50],
            [255, 220, 50],
            [150, 255, 100],
            [100, 220, 255],
            [120, 130, 255],
            [200, 100, 255],
            [255, 100, 190],
        ];
        COLORS[index % COLORS.len()]
    }

    pub fn assign_sample(&mut self, sample: Sample) {
        self.current_frame = sample.in_point as f32;
        self.name = sample.name.clone();
        self.sample = Some(sample);
    }

    pub fn clear(&mut self) {
        self.sample = None;
        self.is_playing = false;
        self.is_triggered = false;
        self.name = format!("Pad {}", self.index + 1);
    }

    fn in_point(&self) -> f32 {
        self.sample
            .as_ref()
            .map(|s| s.in_point as f32)
            .unwrap_or(0.0)
    }

    fn out_point(&self) -> f32 {
        self.sample
            .as_ref()
            .map(|s| s.out_point as f32)
            .unwrap_or(0.0)
    }

    /// Trigger (key down). Behaviour depends on `trigger_mode`.
    pub fn trigger(&mut self) {
        self.is_triggered = true;
        self.trigger_level = 1.0;
        match self.trigger_mode {
            TriggerMode::Gate | TriggerMode::OneShot => {
                self.is_playing = true;
                self.current_frame = self.in_point();
            }
            TriggerMode::Latch => {
                self.is_playing = !self.is_playing;
                if self.is_playing {
                    self.current_frame = self.in_point();
                }
            }
        }
    }

    /// Release (key up) — only Gate stops.
    pub fn release(&mut self) {
        self.is_triggered = false;
        if self.trigger_mode == TriggerMode::Gate {
            self.is_playing = false;
        }
    }

    pub fn stop(&mut self) {
        self.is_playing = false;
        self.is_triggered = false;
        self.current_frame = self.in_point();
    }

    /// Effective speed displayed to the user. In `Synced` mode this is the
    /// rate-match value implied by the tempo/division, not the stored `speed`.
    pub fn effective_speed(&self, bpm: f32) -> f32 {
        match self.playback_mode {
            PlaybackMode::Free => self.speed,
            PlaybackMode::Synced => {
                let bpm = bpm.max(1.0);
                let range = self.out_point() - self.in_point();
                let beats_per_loop =
                    BEAT_DIVISIONS[self.beat_division.clamp(0, BEAT_DIVISIONS.len() - 1)];
                let loop_duration_seconds = beats_per_loop * 60.0 / bpm;
                let native_duration_seconds = range / self.fps().max(1.0);
                if loop_duration_seconds > 0.0 {
                    native_duration_seconds / loop_duration_seconds
                } else {
                    1.0
                }
            }
        }
    }

    fn fps(&self) -> f32 {
        self.sample.as_ref().map(|s| s.fps).unwrap_or(30.0)
    }

    /// Advance playback one frame-tick.
    ///
    /// `beat` is the total number of elapsed beats from the engine tempo clock
    /// (not the per-beat 0–1 phase). In [`PlaybackMode::Synced`] the playhead is
    /// locked to `beat / BEAT_DIVISIONS[division]` so the clip loops over the
    /// selected musical length.
    pub fn update(&mut self, dt: Duration, beat: f32) {
        self.trigger_level = (self.trigger_level - dt.as_secs_f32() * 5.0).max(0.0);
        if !self.is_playing {
            return;
        }
        let Some(sample) = self.sample.as_ref() else {
            self.is_playing = false;
            return;
        };

        match self.playback_mode {
            PlaybackMode::Free => {
                let (frame, playing) = advance(
                    self.current_frame,
                    self.speed,
                    sample.fps,
                    dt.as_secs_f32(),
                    sample.in_point as f32,
                    sample.out_point as f32,
                    self.trigger_mode,
                    self.loop_enabled,
                );
                self.current_frame = frame;
                self.is_playing = playing;
            }
            PlaybackMode::Synced => {
                self.current_frame =
                    synced_frame(beat, self.beat_division, sample.in_point, sample.out_point);
            }
        }
    }

    pub fn color_space(&self) -> ColorSpace {
        self.sample
            .as_ref()
            .map(|s| s.color_space())
            .unwrap_or(ColorSpace::Rgb)
    }

    pub fn has_sample(&self) -> bool {
        self.sample.is_some()
    }

    /// Playback progress 0..1 within the in/out range.
    pub fn progress(&self) -> f32 {
        let Some(s) = self.sample.as_ref() else {
            return 0.0;
        };
        let range = s.out_point.saturating_sub(s.in_point) as f32;
        if range <= 0.0 {
            return 0.0;
        }
        ((self.current_frame - s.in_point as f32) / range).clamp(0.0, 1.0)
    }

    pub fn beat_division_label(&self) -> &'static str {
        BEAT_DIVISION_NAMES[self.beat_division.clamp(0, BEAT_DIVISION_NAMES.len() - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: f32 = 30.0;
    const IN: f32 = 0.0;
    const OUT: f32 = 10.0;

    #[test]
    fn forward_no_cross_keeps_playing() {
        let (f, playing) = advance(2.0, 1.0, FPS, 0.1, IN, OUT, TriggerMode::OneShot, false);
        assert!((f - 5.0).abs() < 1e-3);
        assert!(playing);
    }

    #[test]
    fn oneshot_stops_at_out() {
        let (f, playing) = advance(9.0, 1.0, FPS, 0.1, IN, OUT, TriggerMode::OneShot, false);
        assert!((f - OUT).abs() < 1e-3);
        assert!(!playing); // 9 + 3 = 12 ≥ 10 → clamp to out, stop
    }

    #[test]
    fn latch_wraps_seamlessly() {
        let (f, playing) = advance(9.0, 1.0, FPS, 0.1, IN, OUT, TriggerMode::Latch, false);
        assert!((f - 2.0).abs() < 1e-3); // 12 → in + (12 - 10) = 2
        assert!(playing);
    }

    #[test]
    fn gate_holds_at_out_when_not_looping() {
        let (f, playing) = advance(9.0, 1.0, FPS, 0.1, IN, OUT, TriggerMode::Gate, false);
        assert!((f - OUT).abs() < 1e-3);
        assert!(playing); // held at end, still playing
    }

    #[test]
    fn reverse_oneshot_stops_at_in() {
        let (f, playing) = advance(1.0, -1.0, FPS, 0.1, IN, OUT, TriggerMode::OneShot, false);
        assert!((f - IN).abs() < 1e-3);
        assert!(!playing);
    }

    #[test]
    fn playback_mode_roundtrips() {
        assert_eq!(
            PlaybackMode::from_index(PlaybackMode::Free.to_index()),
            PlaybackMode::Free
        );
        assert_eq!(
            PlaybackMode::from_index(PlaybackMode::Synced.to_index()),
            PlaybackMode::Synced
        );
    }

    #[test]
    fn synced_frame_loops_over_division_beats() {
        // 1/4 note division = 1 beat per loop, range 0..120.
        let f0 = synced_frame(0.0, 2, 0, 120);
        let f_half = synced_frame(0.5, 2, 0, 120);
        let f_one = synced_frame(1.0, 2, 0, 120);
        assert!((f0 - 0.0).abs() < 1e-3);
        assert!((f_half - 60.0).abs() < 1e-3);
        assert!((f_one - 0.0).abs() < 1e-3); // loops back to start after one beat
    }

    #[test]
    fn synced_frame_wraps_beat_phase_across_beats() {
        // Whole note (4 beats) over a range 0..40.
        let f0 = synced_frame(0.0, 4, 0, 40);
        let f1 = synced_frame(1.0, 4, 0, 40);
        let f4 = synced_frame(4.0, 4, 0, 40);
        let f5 = synced_frame(5.0, 4, 0, 40);
        assert!((f0 - 0.0).abs() < 1e-3);
        assert!((f1 - 10.0).abs() < 1e-3);
        assert!((f4 - 0.0).abs() < 1e-3);
        assert!((f5 - 10.0).abs() < 1e-3);
    }

    #[test]
    fn synced_frame_respects_in_out_points() {
        // 1/2 note division (2 beats), range 20..100.
        let f0 = synced_frame(0.0, 3, 20, 100);
        let f1 = synced_frame(1.0, 3, 20, 100);
        let f2 = synced_frame(2.0, 3, 20, 100);
        assert!((f0 - 20.0).abs() < 1e-3);
        assert!((f1 - 60.0).abs() < 1e-3);
        assert!((f2 - 20.0).abs() < 1e-3);
    }
}
