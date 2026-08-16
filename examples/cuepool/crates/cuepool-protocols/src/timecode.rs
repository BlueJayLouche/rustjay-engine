//! Transport-agnostic timecode chasing.
//!
//! [`TimecodeSource`] is the seam between a timecode transport (MTC over MIDI,
//! LTC over audio) and the show: any source produces the same
//! [`TimecodeState`], so the follow logic, drift correction, transport
//! readout, and timecode cue triggers downstream consume it unchanged.

pub use crate::midi::mtc::{MtcFrameRate, SmpteTime, TimecodeState};

/// A source of SMPTE timecode positions for the show to chase.
///
/// Implemented once per transport ([`crate::midi::mtc::MtcReceiver`] for MTC).
/// Called from the engine frame loop; implementations must keep these calls
/// cheap and non-blocking (hot-plug scans are internally throttled).
pub trait TimecodeSource {
    /// Pick up sources connected after startup. Internally throttled.
    fn refresh(&mut self);
    /// Per-frame housekeeping (e.g. clearing `playing` after input silence).
    fn tick(&self);
    /// Snapshot the current timecode state.
    fn clone_state(&self) -> TimecodeState;
}

impl TimecodeSource for crate::midi::mtc::MtcReceiver {
    fn refresh(&mut self) {
        self.refresh();
    }

    fn tick(&self) {
        self.tick();
    }

    fn clone_state(&self) -> TimecodeState {
        self.clone_state()
    }
}
