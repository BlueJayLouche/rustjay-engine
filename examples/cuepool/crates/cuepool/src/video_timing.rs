use std::time::{Duration, Instant};

pub(crate) fn fade_elapsed(start: Instant, pause_started: Option<Instant>) -> Duration {
    pause_started
        .unwrap_or_else(Instant::now)
        .saturating_duration_since(start)
}

pub(crate) fn shift_fade_start_after_pause(
    start: Instant,
    pause_started: Instant,
    resumed_at: Instant,
) -> Instant {
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

        assert_eq!(
            fade_elapsed(origin, Some(paused_at)),
            Duration::from_secs(2)
        );
        let shifted = shift_fade_start_after_pause(origin, paused_at, resumed_at);
        assert_eq!(resumed_at.duration_since(shifted), Duration::from_secs(2));

        let started_while_paused = origin + Duration::from_secs(4);
        assert_eq!(
            fade_elapsed(started_while_paused, Some(paused_at)),
            Duration::ZERO
        );
        let shifted = shift_fade_start_after_pause(started_while_paused, paused_at, resumed_at);
        assert_eq!(shifted, resumed_at);
    }
}
