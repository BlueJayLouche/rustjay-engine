use cuepool_gui::app::CueState;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(crate) struct ActiveCue {
    pub(crate) instance_id: u64,
    pub(crate) qid: rust_decimal::Decimal,
    pub(crate) name: String,
    pub(crate) input: std::sync::Arc<cuepool_audio::MixerInput>,
    pub(crate) state: CueState,
    /// Shared counter incremented by LoopProcessor on each loop boundary.
    pub(crate) loop_counter: Option<std::sync::Arc<std::sync::atomic::AtomicU32>>,
    /// Last known loop count (used to detect new loops).
    pub(crate) video_loop_count: u32,
    /// Loop boundaries in frames, for computing loop-relative position.
    pub(crate) loop_start_frame: u64,
    pub(crate) loop_end_frame: u64,
    /// Tail fade-out (seconds) — begins `fade_out` before the cue's natural end.
    pub(crate) fade_out: f32,
    pub(crate) fade_type: cuepool_core::FadeType,
    pub(crate) fade_out_started: bool,
    /// Stop action scheduled by a StopCue targeting this cue.
    pub(crate) pending_stop: Option<PendingStop>,
}

pub(crate) fn active_cue_length_samples(cue: &ActiveCue) -> Option<usize> {
    let region_frames = cue.loop_end_frame.saturating_sub(cue.loop_start_frame);
    if region_frames > 0 {
        usize::try_from(region_frames)
            .ok()?
            .checked_mul(cue.input.channels())
    } else {
        cue.input.length()
    }
}

/// A cue that is waiting for its delay timer to expire before playing.
pub(crate) struct DelayedCue {
    pub(crate) cue: cuepool_core::Cue,
    pub(crate) start_at: std::time::Instant,
}

/// Pending stop action scheduled by a StopCue with mode != Immediate.
#[derive(Clone, Copy)]
pub(crate) struct PendingStop {
    pub(crate) mode: cuepool_core::StopMode,
    pub(crate) fade_out_time: f32,
    pub(crate) fade_type: cuepool_core::FadeType,
}

pub(crate) fn fade_elapsed(start: Instant, pause_started: Option<Instant>) -> Duration {
    pause_started.unwrap_or_else(Instant::now).saturating_duration_since(start)
}

pub(crate) fn shift_fade_start_after_pause(start: Instant, pause_started: Instant, resumed_at: Instant) -> Instant {
    start + resumed_at.saturating_duration_since(start.max(pause_started))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_fade_freezes_when_paused_before_or_after_it_starts() {
        let origin = Instant::now();
        let paused_at = origin + Duration::from_secs(2);
        let resumed_at = origin + Duration::from_secs(7);

        assert_eq!(fade_elapsed(origin, Some(paused_at)), Duration::from_secs(2));
        let shifted = shift_fade_start_after_pause(origin, paused_at, resumed_at);
        assert_eq!(resumed_at.duration_since(shifted), Duration::from_secs(2));

        let started_while_paused = origin + Duration::from_secs(4);
        assert_eq!(fade_elapsed(started_while_paused, Some(paused_at)), Duration::ZERO);
        let shifted = shift_fade_start_after_pause(started_while_paused, paused_at, resumed_at);
        assert_eq!(shifted, resumed_at);
    }
}
