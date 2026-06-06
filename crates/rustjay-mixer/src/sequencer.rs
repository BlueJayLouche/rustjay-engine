//! Transition sequencer: ordered list of crossfade / hold / effect steps.
//!
//! Implements REQ-05.1–05.3.

use crate::crossfade::{AutoCrossfade, Easing};

/// One step in a transition sequence.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TransitionStep {
    /// What this step does.
    pub kind: StepKind,
}

impl TransitionStep {
    /// Convenience: create a crossfade step.
    pub fn crossfade(target: f32, beats: f32) -> Self {
        Self {
            kind: StepKind::Crossfade { target, beats },
        }
    }

    /// Convenience: create a hold step.
    pub fn hold(beats: f32) -> Self {
        Self {
            kind: StepKind::Hold { beats },
        }
    }

    /// Convenience: create a timed crossfade step (wall-clock seconds).
    pub fn timed_crossfade(target: f32, seconds: f32) -> Self {
        Self {
            kind: StepKind::TimedCrossfade { target, seconds },
        }
    }

    /// Convenience: create a timed hold step (wall-clock seconds).
    pub fn timed_hold(seconds: f32) -> Self {
        Self {
            kind: StepKind::TimedHold { seconds },
        }
    }
}

/// What a single transition step does.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum StepKind {
    /// Crossfade to a target value over N beats.
    Crossfade {
        /// Target crossfader position (0–1).
        target: f32,
        /// Duration in beats.
        beats: f32,
    },
    /// Hold the current crossfader position for N beats.
    Hold {
        /// Duration in beats.
        beats: f32,
    },
    /// Crossfade to a target value over N seconds (wall-clock, independent of BPM).
    TimedCrossfade {
        /// Target crossfader position (0–1).
        target: f32,
        /// Duration in seconds.
        seconds: f32,
    },
    /// Hold the current crossfader position for N seconds.
    TimedHold {
        /// Duration in seconds.
        seconds: f32,
    },
    /// Placeholder for a transition effect (e.g. strobe, wipe).
    ///
    /// Not yet implemented — steps of this kind advance immediately.
    Effect(TransitionEffect),
}

/// Placeholder for transition-specific visual effects.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TransitionEffect;

/// Sequencer state machine.
///
/// Drive with [`SequencerState::tick`] every frame. The sequencer advances
/// through its steps in order, converting beat durations to seconds via the
/// engine BPM. When `looping` is false and the last step completes, the
/// sequencer stops.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SequencerState {
    /// Ordered steps.
    pub steps: Vec<TransitionStep>,
    /// Current step index.
    pub index: usize,
    /// Whether the sequencer is currently playing.
    pub playing: bool,
    /// Whether to loop back to step 0 after the last step.
    pub looping: bool,
    /// True once play() has been called at least once; distinguishes "not yet
    /// started" from "finished" in is_done().
    pub has_run: bool,
    /// Beat accumulator for the current step (used by Hold steps).
    #[serde(skip)]
    step_elapsed_beats: f32,
    /// Active auto-crossfade for a Crossfade or TimedCrossfade step.
    #[serde(skip)]
    auto: Option<AutoCrossfade>,
    /// Wall-clock accumulator for TimedHold steps (seconds).
    #[serde(skip)]
    step_elapsed_seconds: f32,
}

impl SequencerState {
    /// Create an empty, stopped sequencer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start playback from the current step.
    pub fn play(&mut self) {
        self.playing = true;
        self.has_run = true;
    }

    /// Stop playback and reset to step 0.
    pub fn stop(&mut self) {
        self.playing = false;
        self.index = 0;
        self.step_elapsed_beats = 0.0;
        self.step_elapsed_seconds = 0.0;
        self.auto = None;
    }

    /// Pause playback (resume with [`play`](Self::play)).
    pub fn pause(&mut self) {
        self.playing = false;
    }

    /// Advance by `dt` seconds.
    ///
    /// `crossfader` is the current mixer crossfader value. `bpm` is the
    /// effective BPM from the engine (used to convert beat durations to
    /// seconds). Returns `Some(value)` when the crossfader should be updated.
    pub fn tick(&mut self, crossfader: f32, dt: f32, bpm: Option<f32>) -> Option<f32> {
        if !self.playing || self.steps.is_empty() {
            return None;
        }

        let bpm = bpm.unwrap_or(120.0).max(1.0);
        let beats_per_second = bpm / 60.0;
        let dt_beats = dt * beats_per_second;

        let step = &self.steps[self.index];

        match &step.kind {
            StepKind::Crossfade { target, beats } => {
                if self.auto.is_none() {
                    let duration_seconds = beats / beats_per_second;
                    self.auto = Some(AutoCrossfade::new(
                        crossfader,
                        *target,
                        duration_seconds,
                        Easing::EaseInOut,
                    ));
                }

                if let Some(ref mut auto) = self.auto {
                    let target = auto.target();
                    match auto.tick(dt) {
                        Some(v) => return Some(v),
                        None => {
                            self.auto = None;
                            self.advance();
                            return Some(target);
                        }
                    }
                }
            }
            StepKind::Hold { beats } => {
                self.step_elapsed_beats += dt_beats;
                if self.step_elapsed_beats >= *beats {
                    self.step_elapsed_beats -= *beats;
                    self.advance();
                }
            }
            StepKind::TimedCrossfade { target, seconds } => {
                if self.auto.is_none() {
                    self.auto = Some(AutoCrossfade::new(
                        crossfader,
                        *target,
                        *seconds,
                        Easing::EaseInOut,
                    ));
                }

                if let Some(ref mut auto) = self.auto {
                    let target = auto.target();
                    match auto.tick(dt) {
                        Some(v) => return Some(v),
                        None => {
                            self.auto = None;
                            self.advance();
                            return Some(target);
                        }
                    }
                }
            }
            StepKind::TimedHold { seconds } => {
                self.step_elapsed_seconds += dt;
                if self.step_elapsed_seconds >= *seconds {
                    self.step_elapsed_seconds -= *seconds;
                    self.advance();
                }
            }
            StepKind::Effect(_) => {
                // Transition effects are not yet implemented.
                self.advance();
            }
        }

        None
    }

    /// Whether the sequencer has run through all steps and stopped.
    /// Returns false for a sequencer that has never been played.
    pub fn is_done(&self) -> bool {
        self.has_run && !self.playing && self.index == 0 && self.auto.is_none()
    }

    fn advance(&mut self) {
        self.index += 1;
        self.step_elapsed_beats = 0.0;
        self.step_elapsed_seconds = 0.0;
        self.auto = None;
        if self.index >= self.steps.len() {
            if self.looping {
                self.index = 0;
            } else {
                self.playing = false;
                self.index = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequencer_crossfade_and_hold() {
        let mut seq = SequencerState::new();
        seq.steps = vec![
            TransitionStep::crossfade(1.0, 1.0), // 1 beat @ 60 bpm = 1 sec
            TransitionStep::hold(1.0),           // 1 beat @ 60 bpm = 1 sec
        ];
        seq.play();

        // Start crossfade
        let v = seq.tick(0.0, 0.5, Some(60.0));
        assert!(v.is_some());
        // Finish crossfade
        let v = seq.tick(v.unwrap(), 0.6, Some(60.0));
        assert_eq!(v, Some(1.0));
        // Hold step — no crossfader change
        let v = seq.tick(1.0, 0.5, Some(60.0));
        assert_eq!(v, None);
        // Hold done, sequence stops
        let v = seq.tick(1.0, 0.6, Some(60.0));
        assert_eq!(v, None);
        assert!(seq.is_done());
    }

    #[test]
    fn sequencer_loops() {
        let mut seq = SequencerState::new();
        seq.steps = vec![TransitionStep::hold(1.0)];
        seq.looping = true;
        seq.play();

        seq.tick(0.0, 0.6, Some(60.0)); // hold finishes
        assert!(seq.playing); // looping keeps it going
    }

    #[test]
    fn sequencer_stop_clears_state() {
        let mut seq = SequencerState::new();
        seq.steps = vec![TransitionStep::crossfade(1.0, 1.0)];
        seq.play();
        seq.tick(0.0, 0.1, Some(60.0));
        seq.stop();
        assert!(!seq.playing);
        assert_eq!(seq.index, 0);
        assert!(seq.auto.is_none());
    }
}
