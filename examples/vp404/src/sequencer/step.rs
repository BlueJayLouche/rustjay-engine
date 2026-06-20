//! One sequencer step: active gate, velocity, probability, ratchet, gate length.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single step in a sequence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    /// Whether this step is active (contains a trigger).
    pub active: bool,
    /// Velocity/intensity (0.0 - 1.0).
    pub velocity: f32,
    /// Probability of trigger (0.0 - 1.0).
    pub probability: f32,
    /// Number of ratchet repeats (1-8).
    pub ratchet: u8,
    /// Time between ratchets as fraction of step duration.
    pub ratchet_spacing: f32,
    /// Gate length as fraction of step duration (0.0 - 1.0).
    #[serde(default = "default_gate_length")]
    pub gate_length: f32,
    /// Per-step parameter locks (reserved for future use).
    #[serde(default)]
    pub parameter_locks: HashMap<String, f32>,
}

fn default_gate_length() -> f32 {
    0.25
}

impl Step {
    pub fn new() -> Self {
        Self {
            active: false,
            velocity: 1.0,
            probability: 1.0,
            ratchet: 1,
            ratchet_spacing: 0.5,
            gate_length: default_gate_length(),
            parameter_locks: HashMap::new(),
        }
    }

    pub fn active() -> Self {
        Self {
            active: true,
            ..Self::new()
        }
    }

    pub fn toggle(&mut self) {
        self.active = !self.active;
    }

    pub fn should_trigger(&self) -> bool {
        if !self.active {
            return false;
        }
        if self.probability >= 1.0 {
            return true;
        }
        rand::random::<f32>() < self.probability
    }
}

impl Default for Step {
    fn default() -> Self {
        Self::new()
    }
}

/// Tiny deterministic RNG for probability checks.
pub mod rand {
    use std::cell::Cell;

    thread_local! {
        static RNG: Cell<u64> = const { Cell::new(0x123456789abcdef0) };
    }

    pub fn random<T>() -> T
    where
        T: Random,
    {
        T::random()
    }

    pub trait Random {
        fn random() -> Self;
    }

    impl Random for f32 {
        fn random() -> Self {
            RNG.with(|rng| {
                let old = rng.get();
                let mut x = old;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                rng.set(x);
                (x as f64 / u64::MAX as f64) as f32
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_toggles() {
        let mut s = Step::new();
        assert!(!s.active);
        s.toggle();
        assert!(s.active);
        s.toggle();
        assert!(!s.active);
    }

    #[test]
    fn active_step_always_triggers_when_probability_one() {
        let s = Step::active();
        assert!(s.should_trigger());
    }

    #[test]
    fn inactive_step_never_triggers() {
        let s = Step::new();
        assert!(!s.should_trigger());
    }
}
