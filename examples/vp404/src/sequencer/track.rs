//! One sequencer track (one pad) with step data, gate tracking, and mutes.

use serde::{Deserialize, Serialize};

use super::step::Step;

/// An active gate that will release a pad at a future beat.
#[derive(Debug, Clone, Copy)]
pub struct ActiveGate {
    pub end_beat: f32,
    pub step_index: usize,
}

/// A sequencer track controls one pad.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub pad_index: usize,
    pub steps: Vec<Step>,
    pub length: usize,
    #[serde(skip)]
    pub current_step: usize,
    #[serde(skip)]
    pub is_playing: bool,
    pub muted: bool,
    pub solo: bool,
    pub probability_override: Option<f32>,
    pub name: Option<String>,
    #[serde(skip)]
    pub active_gates: Vec<ActiveGate>,
}

impl Track {
    pub fn new(pad_index: usize) -> Self {
        Self {
            pad_index,
            steps: vec![Step::new(); 64],
            length: 16,
            current_step: 0,
            is_playing: false,
            muted: false,
            solo: false,
            probability_override: None,
            name: None,
            active_gates: Vec::new(),
        }
    }

    pub fn current(&self) -> &Step {
        &self.steps[self.current_step]
    }

    pub fn current_mut(&mut self) -> &mut Step {
        &mut self.steps[self.current_step]
    }

    pub fn get_step(&self, index: usize) -> Option<&Step> {
        self.steps.get(index)
    }

    pub fn get_step_mut(&mut self, index: usize) -> Option<&mut Step> {
        self.steps.get_mut(index)
    }

    pub fn set_length(&mut self, length: usize) {
        self.length = length.clamp(1, self.steps.len());
        if self.current_step >= self.length {
            self.current_step = 0;
        }
    }

    pub fn toggle_step(&mut self, step: usize) {
        if let Some(s) = self.steps.get_mut(step) {
            s.toggle();
        }
    }

    pub fn clear(&mut self) {
        for step in &mut self.steps {
            *step = Step::new();
        }
        self.current_step = 0;
    }

    pub fn should_trigger(&self) -> bool {
        if self.muted {
            return false;
        }
        let step = self.current();
        if let Some(prob) = self.probability_override {
            if prob <= 0.0 {
                return false;
            }
            if prob < 1.0 && super::step::rand::random::<f32>() > prob {
                return false;
            }
        }
        step.should_trigger()
    }

    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("Pad {}", self.pad_index + 1))
    }
}

impl Default for Track {
    fn default() -> Self {
        Self::new(0)
    }
}
