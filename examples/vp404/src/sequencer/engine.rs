//! Sequencer engine: polyphonic step sequencer slaved to the engine beat phase.
//!
//! 404's standalone `sequencer/clock.rs` is intentionally dropped; tempo and
//! phase come from `EngineState::effective_beat_phase()` / `effective_bpm()`.

use serde::{Deserialize, Serialize};

use super::pattern::Pattern;
use super::track::ActiveGate;
use crate::bank::{BankHandle, PadCmd};

/// Events emitted by the sequencer for UI feedback.
#[derive(Debug, Clone)]
pub enum SequencerEvent {
    Trigger { pad: usize, velocity: f32 },
    Release { pad: usize },
    StepAdvance { track: usize, step: usize },
    PatternChange { from: usize, to: usize },
}

/// Quantization modes for live recording (reserved).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuantizeMode {
    Off,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
}

/// Sequencer engine. Store this in `Vp404State` for preset persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequencerEngine {
    pub patterns: Vec<Pattern>,
    pub current_pattern: usize,
    #[serde(skip)]
    pub queued_pattern: Option<usize>,
    #[serde(skip)]
    pub is_playing: bool,
    pub quantize_mode: QuantizeMode,
    /// Playhead in beats — sequencer-owned; advances only while playing, so it
    /// can be reset/rewound. (Was previously slaved to the global free-running
    /// clock, which made Reset impossible.)
    #[serde(skip)]
    pub position: f32,
    /// Last absolute beat seen from the global clock, for per-frame deltas.
    /// Tracked even while stopped so resuming never produces a catch-up burst.
    #[serde(skip)]
    pub last_clock: f32,
    #[serde(skip)]
    pub current_bar: u32,
    #[serde(skip)]
    pub events: Vec<SequencerEvent>,
}

/// A 16th note is one quarter of a beat.
const BEATS_PER_STEP: f32 = 0.25;
/// A bar is four beats.
const BEATS_PER_BAR: f32 = 4.0;

impl SequencerEngine {
    pub fn new(pattern_count: usize) -> Self {
        Self {
            patterns: (0..pattern_count).map(Pattern::new).collect(),
            current_pattern: 0,
            queued_pattern: None,
            is_playing: false,
            quantize_mode: QuantizeMode::Sixteenth,
            position: 0.0,
            last_clock: 0.0,
            current_bar: 0,
            events: Vec::with_capacity(32),
        }
    }

    pub fn play(&mut self) {
        self.is_playing = true;
        self.rewind();
        log::info!("VP-404 sequencer: play");
    }

    /// Return the playhead to the downbeat. Starts a hair *before* step 0 so the
    /// first advance crosses into it and step 0 fires on the beat.
    fn rewind(&mut self) {
        self.position = -1e-4;
        self.current_bar = 0;
        for track in &mut self.current_pattern_mut().tracks {
            track.current_step = 0;
            track.active_gates.clear();
        }
    }

    pub fn stop(&mut self) {
        self.is_playing = false;
        // Release any held gates.
        for track in &mut self.current_pattern_mut().tracks {
            track.active_gates.clear();
        }
        log::info!("VP-404 sequencer: stop");
    }

    pub fn toggle_playback(&mut self) {
        if self.is_playing {
            self.stop();
        } else {
            self.play();
        }
    }

    pub fn reset_position(&mut self) {
        // Deliberately does NOT touch `last_clock` — the external clock keeps
        // running, so the next delta stays small rather than snapping the
        // playhead to wherever the clock currently is.
        self.rewind();
    }

    /// Hard reset: rewind the playhead AND sync `last_clock` to `clock`.
    ///
    /// Use this when the *external* clock is also being reset (shift+space).
    /// The next `tick(clock)` will produce a delta ≈ 0 instead of a large
    /// negative value that would stall the sequencer.
    pub fn reset_with_clock(&mut self, clock: f32) {
        self.rewind();
        self.last_clock = clock;
    }

    pub fn current_pattern(&self) -> &Pattern {
        &self.patterns[self.current_pattern]
    }

    pub fn current_pattern_mut(&mut self) -> &mut Pattern {
        &mut self.patterns[self.current_pattern]
    }

    pub fn queue_pattern(&mut self, index: usize) {
        if index < self.patterns.len() {
            self.queued_pattern = Some(index);
        }
    }

    pub fn switch_pattern(&mut self, index: usize) {
        if index >= self.patterns.len() || index == self.current_pattern {
            return;
        }
        let old = self.current_pattern;
        // Stop currently playing pads from the old pattern.
        for track in &mut self.patterns[old].tracks {
            track.is_playing = false;
            track.active_gates.clear();
        }
        self.current_pattern = index;
        for track in &mut self.current_pattern_mut().tracks {
            track.current_step = 0;
            track.is_playing = true;
        }
        self.events.push(SequencerEvent::PatternChange {
            from: old,
            to: index,
        });
        log::info!("VP-404 sequencer: switched to pattern {}", index + 1);
    }

    pub fn toggle_step(&mut self, track: usize, step: usize) {
        if let Some(t) = self.current_pattern_mut().get_track_mut(track) {
            t.toggle_step(step);
        }
    }

    pub fn set_step(&mut self, track: usize, step: usize, active: bool) {
        if let Some(t) = self.current_pattern_mut().get_track_mut(track) {
            if let Some(s) = t.steps.get_mut(step) {
                s.active = active;
            }
        }
    }

    pub fn mute_track(&mut self, track: usize) {
        if let Some(t) = self.current_pattern_mut().get_track_mut(track) {
            t.muted = true;
        }
    }

    pub fn unmute_track(&mut self, track: usize) {
        if let Some(t) = self.current_pattern_mut().get_track_mut(track) {
            t.muted = false;
        }
    }

    pub fn clear_pattern(&mut self) {
        self.current_pattern_mut().clear();
    }

    /// Advance the sequencer by `beat_delta` beats, posting pad commands via the
    /// shared handle. `beat` is the engine's accumulated beat count.
    pub fn tick(&mut self, clock_beat: f32, handle: &BankHandle) {
        self.events.clear();

        // The global clock is free-running; track it every frame (even while
        // stopped) so the per-frame delta stays small and resuming/resetting
        // never replays a burst of missed steps.
        let delta = clock_beat - self.last_clock;
        self.last_clock = clock_beat;

        if !self.is_playing || delta <= 0.0 {
            return;
        }

        // Advance the sequencer-owned playhead by the elapsed musical time.
        let prev_beat = self.position;
        self.position += delta;
        let beat = self.position;

        // Pattern switching on the next bar boundary.
        let prev_bar = (prev_beat / BEATS_PER_BAR).floor() as u32;
        let curr_bar = (beat / BEATS_PER_BAR).floor() as u32;
        if curr_bar != prev_bar {
            self.current_bar = curr_bar;
            if let Some(queued) = self.queued_pattern.take() {
                self.switch_pattern(queued);
            }
        }

        let pattern_len = self.current_pattern().length();
        let mut pending_events: Vec<SequencerEvent> = Vec::with_capacity(32);

        // Gate releases.
        {
            let pattern = self.current_pattern_mut();
            for (track_idx, track) in pattern.tracks.iter_mut().enumerate() {
                let mut released = false;
                track.active_gates.retain(|gate| {
                    if beat >= gate.end_beat {
                        released = true;
                        false
                    } else {
                        true
                    }
                });
                if released {
                    handle.post(PadCmd::Release(track_idx));
                    pending_events.push(SequencerEvent::Release { pad: track_idx });
                }
            }
        }

        // Step triggers: examine every 16th-note tick crossed since last frame.
        let start_step = (prev_beat / BEATS_PER_STEP).floor() as i64;
        let end_step = (beat / BEATS_PER_STEP).floor() as i64;

        for step_index in (start_step + 1)..=end_step {
            let step_in_pattern = step_index.rem_euclid(pattern_len as i64) as usize;

            let pattern = self.current_pattern_mut();
            for (track_idx, track) in pattern.tracks.iter_mut().enumerate() {
                let was = track.current_step;
                track.current_step = step_in_pattern;
                if track.current_step != was {
                    pending_events.push(SequencerEvent::StepAdvance {
                        track: track_idx,
                        step: track.current_step,
                    });
                }

                if track.should_trigger() {
                    let step = &track.steps[track.current_step];
                    // gate_length is measured in steps: <1 = short gate within a
                    // step, >1 = a tied gate spanning several steps.
                    let gate_beats = step.gate_length.max(0.0) * BEATS_PER_STEP;
                    track.active_gates.push(ActiveGate {
                        end_beat: beat + gate_beats,
                        step_index: track.current_step,
                    });

                    let ratchets = step.ratchet.clamp(1, 8);
                    for _ in 0..ratchets {
                        handle.post(PadCmd::Trigger(track_idx));
                        pending_events.push(SequencerEvent::Trigger {
                            pad: track_idx,
                            velocity: step.velocity,
                        });
                    }
                }
            }
        }

        self.events.extend(pending_events);
    }
}

impl Default for SequencerEngine {
    fn default() -> Self {
        Self::new(16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::BankHandle;

    #[test]
    fn toggle_step_and_trigger() {
        let mut seq = SequencerEngine::new(4);
        seq.toggle_step(0, 0);
        assert!(seq.current_pattern().tracks[0].steps[0].active);

        let handle = BankHandle::new();
        seq.play(); // rewinds to just before step 0
        seq.tick(0.1, &handle); // advance into step 0 → fires
        assert!(seq
            .events
            .iter()
            .any(|e| matches!(e, SequencerEvent::Trigger { pad: 0, .. })));

        let cmds = handle.cmds.lock().unwrap();
        assert!(cmds.iter().any(|c| matches!(c, PadCmd::Trigger(0))));
    }

    #[test]
    fn gate_releases_after_fraction_of_beat() {
        let mut seq = SequencerEngine::new(4);
        seq.toggle_step(0, 0);
        seq.current_pattern_mut().tracks[0].steps[0].gate_length = 0.5; // 1/32 note gate
        let handle = BankHandle::new();
        seq.play();

        seq.tick(0.05, &handle); // crosses step 0 → trigger; gate end ≈ 0.05 + 0.125
        assert!(!seq.current_pattern().tracks[0].active_gates.is_empty());

        seq.tick(0.10, &handle); // before gate end
        assert!(!seq.current_pattern().tracks[0].active_gates.is_empty());

        seq.tick(0.40, &handle); // after gate end
        assert!(seq.current_pattern().tracks[0].active_gates.is_empty());
    }

    #[test]
    fn multi_step_gate_holds_across_steps() {
        let mut seq = SequencerEngine::new(4);
        seq.toggle_step(0, 0);
        // Tie the gate across two steps (gate_length is now in step units).
        seq.current_pattern_mut().tracks[0].steps[0].gate_length = 2.0;
        let handle = BankHandle::new();
        seq.play();

        seq.tick(0.05, &handle); // cross step 0 → trigger; gate end ≈ 0.05 + 2*0.25
        assert!(!seq.current_pattern().tracks[0].active_gates.is_empty());

        seq.tick(0.40, &handle); // playhead now in step 1, but gate still held
        assert!(!seq.current_pattern().tracks[0].active_gates.is_empty());

        seq.tick(0.60, &handle); // past the tied gate end → released
        assert!(seq.current_pattern().tracks[0].active_gates.is_empty());
    }

    #[test]
    fn pattern_switch_on_bar_boundary() {
        let mut seq = SequencerEngine::new(4);
        seq.queue_pattern(1);
        seq.is_playing = true;
        seq.tick(3.9, &BankHandle::new());
        assert_eq!(seq.current_pattern, 0);
        seq.tick(4.1, &BankHandle::new());
        assert_eq!(seq.current_pattern, 1);
    }

    /// Reproduces the live wiring: the plugin feeds a free-running, ever-growing
    /// `accumulated_beats`; while stopped, `tick` just tracks it. Pressing Play
    /// must then advance the playhead and fire steps from that offset.
    #[test]
    fn play_from_running_clock_fires() {
        let mut seq = SequencerEngine::new(1);
        seq.toggle_step(0, 0); // track 0, step 0 active
        let h = BankHandle::new();

        // Free clock runs for a while with the sequencer stopped.
        seq.tick(10.0, &h);
        seq.tick(20.0, &h);
        assert!(h.cmds.lock().unwrap().is_empty(), "fired while stopped");
        let start_playhead = seq.position;

        // User presses Play, then the clock keeps advancing each frame.
        seq.play();
        for beat in [20.1f32, 20.3, 21.0, 24.1] {
            seq.tick(beat, &h);
        }

        assert!(seq.position > start_playhead, "playhead did not advance");
        assert!(
            h.cmds.lock().unwrap().iter().any(|c| matches!(c, PadCmd::Trigger(0))),
            "no Trigger fired after Play from a running clock"
        );
    }

    /// The reported bug: Reset must return the playhead to step 0 and HOLD there,
    /// even though the global clock is at a large value.
    #[test]
    fn reset_holds_against_running_clock() {
        let mut seq = SequencerEngine::new(1);
        let h = BankHandle::new();

        seq.tick(100.0, &h); // free clock already far along
        seq.play();
        seq.tick(100.5, &h); // playhead advances ~0.5
        assert!(seq.position > 0.3);

        seq.reset_position();
        assert!(seq.position <= 0.0); // rewound to the downbeat

        // The next frame advances by only the small delta — the playhead stays
        // near 0 instead of snapping back to the global clock's 100+.
        seq.tick(100.6, &h);
        assert!(seq.position < 0.2, "reset did not hold: position = {}", seq.position);
    }
}
