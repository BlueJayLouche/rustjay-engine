//! VP-404 API state schema (app-owned).
//!
//! `step_write` is always compiled (MIDI step-record works without `--features api`).
//! Snapshot types, SeqCmd, and build_snapshot are gated by `#[cfg(feature = "api")]`
//! since they're only used by the web layer. No types here leak into shared crates.

use crate::sequencer::SequencerEngine;

// ── Always-compiled: step-write helper ───────────────────────────────

/// Record track `track` as active at `edit_step`, then advance the cursor
/// (wrapping at `pattern.length()`). Returns the new cursor position.
pub fn step_write(seq: &mut SequencerEngine, track: usize, edit_step: usize) -> usize {
    let pattern_len = seq
        .patterns
        .get(seq.current_pattern)
        .map(|p| p.length())
        .unwrap_or(16);
    seq.set_step(track, edit_step, true);
    (edit_step + 1) % pattern_len.max(1)
}

// ── API-feature-gated types ───────────────────────────────────────────

#[cfg(feature = "api")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "api")]
use crate::bank::{Bank, PAD_COUNT};

/// Top-level snapshot published into `EngineState::app_state` each frame.
#[cfg(feature = "api")]
#[derive(Debug, Clone, Serialize)]
pub struct Vp404Snapshot {
    pub pads: Vec<PadSnapshot>,
    pub current_pattern: usize,
    /// Current playhead step (0-based), or None when stopped.
    pub sequencer_step: Option<usize>,
    pub sequencer_playing: bool,
    pub pattern_length: usize,
    /// Active steps: [track][step].
    pub steps: Vec<Vec<bool>>,
    /// MIDI step-write cursor position (0-based).
    pub edit_step: usize,
    /// Whether step-write record mode is active.
    pub record_mode: bool,
}

/// Per-pad state visible to web clients.
#[cfg(feature = "api")]
#[derive(Debug, Clone, Serialize)]
pub struct PadSnapshot {
    pub name: String,
    pub color: [u8; 3],
    pub loaded: bool,
    pub playing: bool,
    pub progress: f32,
}

/// Sequencer commands posted via `POST /api/app/command` and drained in
/// `prepare()`. VP-404-internal — never exposed in shared crates.
#[cfg(feature = "api")]
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SeqCmd {
    ToggleStep { track: usize, step: usize },
    SetStep { track: usize, step: usize, active: bool },
    SetLength { steps: usize },
    SelectPattern { index: usize },
    /// Move the MIDI step-write cursor to a specific position.
    SetEditStep { step: usize },
    /// Transport controls.
    Play,
    Stop,
    TogglePlay,
    ResetPosition,
    /// Enable/disable MIDI step-write record mode.
    SetRecord { enabled: bool },
}

#[cfg(feature = "api")]
impl SeqCmd {
    pub fn apply(self, seq: &mut SequencerEngine, edit_step: &mut usize) {
        match self {
            SeqCmd::ToggleStep { track, step } => seq.toggle_step(track, step),
            SeqCmd::SetStep { track, step, active } => seq.set_step(track, step, active),
            SeqCmd::SetLength { steps } => {
                if let Some(pat) = seq.patterns.get_mut(seq.current_pattern) {
                    pat.set_length(steps.clamp(1, 64));
                }
            }
            SeqCmd::SelectPattern { index } => seq.queue_pattern(index),
            SeqCmd::SetEditStep { step } => {
                let max = seq
                    .patterns
                    .get(seq.current_pattern)
                    .map(|p| p.length())
                    .unwrap_or(16);
                *edit_step = step.min(max.saturating_sub(1));
            }
            SeqCmd::Play => seq.play(),
            SeqCmd::Stop => seq.stop(),
            SeqCmd::TogglePlay => seq.toggle_playback(),
            SeqCmd::ResetPosition => seq.reset_position(),
            // Handled by the caller (needs plugin state, not sequencer state).
            SeqCmd::SetRecord { .. } => {}
        }
    }
}

/// Build a snapshot from the live bank + sequencer state.
#[cfg(feature = "api")]
pub fn build_snapshot(
    bank: &Bank,
    seq: &SequencerEngine,
    edit_step: usize,
    record_mode: bool,
) -> Vp404Snapshot {
    let pads = bank
        .pads
        .iter()
        .take(PAD_COUNT)
        .map(|p| PadSnapshot {
            name: p.name.clone(),
            color: p.color,
            loaded: p.has_sample(),
            playing: p.is_playing,
            progress: p.progress(),
        })
        .collect();

    let pattern = seq.patterns.get(seq.current_pattern);
    let pattern_length = pattern.map(|p| p.length()).unwrap_or(16);

    let steps: Vec<Vec<bool>> = pattern
        .map(|p| {
            p.tracks
                .iter()
                .map(|t| t.steps.iter().take(pattern_length).map(|s| s.active).collect())
                .collect()
        })
        .unwrap_or_default();

    let sequencer_step = if seq.is_playing {
        Some(((seq.position / 0.25) as usize) % pattern_length.max(1))
    } else {
        None
    };

    Vp404Snapshot {
        pads,
        current_pattern: seq.current_pattern,
        sequencer_step,
        sequencer_playing: seq.is_playing,
        pattern_length,
        steps,
        edit_step,
        record_mode,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequencer::SequencerEngine;

    #[test]
    fn step_write_records_and_advances() {
        let mut seq = SequencerEngine::new(1);
        let cursor = step_write(&mut seq, 0, 0);
        assert!(seq.current_pattern().tracks[0].steps[0].active);
        assert_eq!(cursor, 1);

        let cursor = step_write(&mut seq, 1, cursor);
        assert!(seq.current_pattern().tracks[1].steps[1].active);
        assert_eq!(cursor, 2);
    }

    #[test]
    fn step_write_wraps_at_pattern_length() {
        let mut seq = SequencerEngine::new(1);
        seq.patterns[0].set_length(4);
        let cursor = step_write(&mut seq, 0, 3);
        assert_eq!(cursor, 0);
    }

    #[cfg(feature = "api")]
    #[test]
    fn seq_cmd_toggle_step() {
        let mut seq = SequencerEngine::new(1);
        let mut edit = 0;
        SeqCmd::ToggleStep { track: 0, step: 5 }.apply(&mut seq, &mut edit);
        assert!(seq.current_pattern().tracks[0].steps[5].active);
        SeqCmd::ToggleStep { track: 0, step: 5 }.apply(&mut seq, &mut edit);
        assert!(!seq.current_pattern().tracks[0].steps[5].active);
    }

    #[cfg(feature = "api")]
    #[test]
    fn seq_cmd_set_length() {
        let mut seq = SequencerEngine::new(1);
        let mut edit = 0;
        SeqCmd::SetLength { steps: 32 }.apply(&mut seq, &mut edit);
        assert_eq!(seq.current_pattern().length(), 32);
    }
}
