//! Crossfade state machines: auto-interpolated and beat-synced.
//!
//! Implements REQ-04.1–04.4 (auto-crossfade + beat-synced crossfade).

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl Easing {
    pub fn apply(self, t: f32) -> f32 {
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t,
            Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Easing::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
        }
    }
}

/// When [`tick`](Self::tick) returns `None` the crossfade is finished; snap the crossfader to the target value.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AutoCrossfade {
    from: f32,
    to: f32,
    duration: f32,
    elapsed: f32,
    easing: Easing,
}

impl AutoCrossfade {
    pub fn new(from: f32, to: f32, duration: f32, easing: Easing) -> Self {
        Self {
            from,
            to,
            duration: duration.max(0.001),
            elapsed: 0.0,
            easing,
        }
    }

    /// Returns `None` when the crossfade has reached its target.
    pub fn tick(&mut self, dt: f32) -> Option<f32> {
        self.elapsed += dt;
        let t = (self.elapsed / self.duration).clamp(0.0, 1.0);
        let value = self.from + (self.to - self.from) * self.easing.apply(t);
        if t >= 1.0 {
            None
        } else {
            Some(value)
        }
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    pub fn is_active(&self) -> bool {
        self.elapsed < self.duration
    }
}

/// Waits for the next beat boundary, then runs an [`AutoCrossfade`]
/// whose duration is derived from `beats × 60 / bpm`.
#[derive(Clone, Debug)]
pub struct BeatSyncCrossfade {
    pub target: f32,
    pub beats: f32,
    /// Whether the beat-sync wait has completed and the auto-crossfade has started.
    started: bool,
    /// The underlying auto-crossfade, created once the beat boundary is hit.
    auto: Option<AutoCrossfade>,
}

impl BeatSyncCrossfade {
    pub fn new(target: f32, beats: f32) -> Self {
        Self {
            target,
            beats,
            started: false,
            auto: None,
        }
    }

    /// Advance by `dt` seconds.
    ///
    /// `current` is the current crossfader value (used as the starting point
    /// once the beat boundary is reached). `bpm` and `beat_phase` come from
    /// the engine's sync state.
    ///
    /// Returns `Some(value)` while the crossfade is in progress.
    /// Returns `None` when finished or still waiting for the beat boundary.
    pub fn tick(
        &mut self,
        current: f32,
        dt: f32,
        bpm: Option<f32>,
        beat_phase: f32,
    ) -> Option<f32> {
        if let (false, true, Some(bpm)) = (self.started, beat_phase < 0.05, bpm) {
            let duration = self.beats * 60.0 / bpm.max(1.0);
            self.auto = Some(AutoCrossfade::new(
                current,
                self.target,
                duration,
                Easing::EaseInOut,
            ));
            self.started = true;
        }

        if let Some(ref mut auto) = self.auto {
            match auto.tick(dt) {
                Some(v) => Some(v),
                None => {
                    // Done — snap to target and clear.
                    self.auto = None;
                    Some(self.target)
                }
            }
        } else {
            None
        }
    }

    pub fn is_done(&self) -> bool {
        self.started && self.auto.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_crossfade_linear() {
        let mut auto = AutoCrossfade::new(0.0, 1.0, 1.0, Easing::Linear);
        assert!((auto.tick(0.0).unwrap() - 0.0).abs() < 1e-6);
        assert!((auto.tick(0.5).unwrap() - 0.5).abs() < 1e-6);
        assert!(auto.tick(0.5).is_none()); // done
        assert_eq!(auto.target(), 1.0);
    }

    #[test]
    fn auto_crossfade_snaps_to_target() {
        let mut auto = AutoCrossfade::new(0.0, 1.0, 0.1, Easing::Linear);
        assert!(auto.tick(0.2).is_none());
    }

    #[test]
    fn beat_sync_waits_for_beat_boundary() {
        let mut bs = BeatSyncCrossfade::new(1.0, 4.0);
        // beat_phase = 0.5, not near boundary → still waiting
        assert_eq!(bs.tick(0.0, 0.1, Some(120.0), 0.5), None);
        assert!(!bs.is_done());
        // beat_phase = 0.0, near boundary → starts
        let v = bs.tick(0.0, 0.1, Some(120.0), 0.0);
        assert!(v.is_some());
        // Finish it
        let v = bs.tick(0.0, 10.0, Some(120.0), 0.0);
        assert_eq!(v, Some(1.0));
        assert!(bs.is_done());
    }
}
