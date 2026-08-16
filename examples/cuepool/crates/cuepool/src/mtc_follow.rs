//! MTC-follow decision logic for video cues.
//!
//! Pure functions + state, kept free of wgpu/audio so the sync policy is
//! unit-testable. `main.rs` owns the side effects (clock re-anchors, decode
//! seeks) driven by [`drift_action`]'s verdict.

use std::time::Instant;

/// What to do about a measured drift between the MTC target position and the
/// video's current position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MtcAdjust {
    /// Within the deadband — do nothing.
    None,
    /// Shift the video position by this many seconds this tick (clock slew,
    /// no decoder involvement).
    Nudge(f64),
    /// Drift too large to slew out — re-seek the decoder and re-anchor.
    HardSync,
    /// No valid elapsed-time reference — hold position this tick.
    Hold,
}

/// Drift below this is presentation jitter — ignored.
pub const DEADBAND_SECS: f64 = 0.040;
/// Drift above this can't be slewed out inaudibly — seek instead.
pub const HARD_SYNC_SECS: f64 = 0.250;
/// Max clock slew per unit elapsed realtime while nudging (5 %).
pub const MAX_SLEW: f64 = 0.05;

/// Floor between hard-sync media reopens. Every hard sync is a full container
/// open (index parse included) and the follow runs each tick, so a scrubbing
/// source would otherwise drive a back-to-back stream of opens.
pub const HARD_SYNC_REOPEN_FLOOR: std::time::Duration = std::time::Duration::from_millis(250);

/// What a stopped or locating source should do this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocateAction {
    /// Reopen the decoder at the target.
    pub sync: bool,
    /// Freeze the follow at the target.
    pub hold: bool,
}

/// Decide the locate behaviour for a source that is running but not playing.
///
/// `may_reopen` is false while a previous hard sync is still inside
/// [`HARD_SYNC_REOPEN_FLOOR`]. A coalesced sync must NOT hold: holding writes
/// the target into the position the next tick compares against, so the
/// remaining drift would disappear and the scrub's final position would never
/// be applied. Coalescing is safe only because the next eligible tick syncs
/// to the then-current target.
pub fn locate_action(drift: f64, may_reopen: bool) -> LocateAction {
    if drift.abs() <= DEADBAND_SECS {
        // Already there — nothing to reopen, safe to freeze.
        return LocateAction {
            sync: false,
            hold: true,
        };
    }
    LocateAction {
        sync: may_reopen,
        hold: may_reopen,
    }
}

/// Decide how to correct `drift` (target − current video position, seconds)
/// measured over `dt` seconds of elapsed realtime.
///
/// - `|drift| <= 40 ms` → [`MtcAdjust::None`]
/// - `40 ms < |drift| <= 250 ms` → [`MtcAdjust::Nudge`] with the drift clamped
///   to ±5 % of `dt` (proportional below the cap)
/// - `|drift| > 250 ms` → [`MtcAdjust::HardSync`]
pub fn drift_action(drift: f64, dt: f64) -> MtcAdjust {
    if dt <= 0.0 {
        return MtcAdjust::Hold;
    }
    let mag = drift.abs();
    if mag <= DEADBAND_SECS {
        MtcAdjust::None
    } else if mag <= HARD_SYNC_SECS {
        let cap = MAX_SLEW * dt;
        MtcAdjust::Nudge(drift.clamp(-cap, cap))
    } else {
        MtcAdjust::HardSync
    }
}

/// Engine-side state for the one video cue currently following MTC.
pub struct MtcFollowState {
    /// QID of the follow cue (its video is the one being driven).
    pub qid: rust_decimal::Decimal,
    /// Media path, needed to re-spawn the decoder on hard syncs.
    pub path: String,
    /// MTC position (seconds) that maps to the video's start (`mtc_start`).
    pub offset_secs: f64,
    /// While Some, MTC is stopped/locating and the video freezes on this
    /// position (`video_paused_position` returns it).
    pub hold_position: Option<f64>,
    /// Last time `drive_mtc_follow` ran — `dt` for the slew cap.
    pub last_tick: Instant,
    /// Last MTC position seen (seconds, before the start offset). MTC only
    /// publishes a complete timecode every 2 frames, so the target between
    /// updates is extrapolated from this + `last_mtc_at`.
    pub last_mtc_secs: f64,
    /// When `last_mtc_secs` last changed.
    pub last_mtc_at: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_holds_only_once_the_picture_is_there() {
        // Within the deadband: nothing to reopen, and freezing is correct.
        assert_eq!(
            locate_action(0.01, true),
            LocateAction {
                sync: false,
                hold: true
            }
        );
        // Off target and allowed: reopen and freeze there.
        assert_eq!(
            locate_action(2.0, true),
            LocateAction {
                sync: true,
                hold: true
            }
        );
        // Off target but coalesced: must not freeze, or the next tick sees no
        // drift and the scrub's final position is never applied.
        assert_eq!(
            locate_action(2.0, false),
            LocateAction {
                sync: false,
                hold: false
            }
        );
    }

    #[test]
    fn deadband_does_nothing() {
        assert_eq!(drift_action(0.039, 1.0), MtcAdjust::None);
        assert_eq!(drift_action(-0.040, 1.0), MtcAdjust::None);
        assert_eq!(drift_action(0.0, 1.0), MtcAdjust::None);
    }

    #[test]
    fn small_drift_nudges_proportionally() {
        // 60 ms drift with a 1 s tick: 5 % cap = 50 ms — full drift not applied.
        assert_eq!(drift_action(0.060, 1.0), MtcAdjust::Nudge(0.050));
        // 45 ms drift, 1 s tick: under the cap, applied as-is.
        assert_eq!(drift_action(0.045, 1.0), MtcAdjust::Nudge(0.045));
    }

    #[test]
    fn nudge_rate_is_capped_by_dt() {
        // Short tick (16 ms frame): cap = 0.8 ms, even though drift is 100 ms.
        assert_eq!(drift_action(0.100, 0.016), MtcAdjust::Nudge(0.0008));
        assert_eq!(drift_action(-0.100, 0.016), MtcAdjust::Nudge(-0.0008));
    }

    #[test]
    fn sign_handling_both_directions() {
        assert_eq!(drift_action(-0.045, 1.0), MtcAdjust::Nudge(-0.045));
        assert_eq!(drift_action(-0.060, 1.0), MtcAdjust::Nudge(-0.050));
    }

    #[test]
    fn big_drift_hard_syncs() {
        assert_eq!(drift_action(0.251, 1.0), MtcAdjust::HardSync);
        assert_eq!(drift_action(-5.0, 1.0), MtcAdjust::HardSync);
        // Boundary: exactly 250 ms still nudges.
        assert_eq!(drift_action(0.250, 1.0), MtcAdjust::Nudge(0.050));
    }

    #[test]
    fn zero_dt_holds() {
        assert_eq!(drift_action(0.100, 0.0), MtcAdjust::Hold);
        assert_eq!(drift_action(0.100, -1.0), MtcAdjust::Hold);
    }
}
