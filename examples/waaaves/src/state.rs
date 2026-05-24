use serde::{Deserialize, Serialize};

use crate::params::{Block1Params, Block2Params, Block3Params};

#[derive(Serialize, Deserialize)]
pub struct WaaavesState {
    pub block1: Block1Params,
    pub block2: Block2Params,
    pub block3: Block3Params,
    #[serde(default = "default_max_delay_frames")]
    pub max_delay_frames: u32,
    #[serde(skip)]
    pub pick_state: PickState,
}

impl Default for WaaavesState {
    fn default() -> Self {
        Self {
            block1: Block1Params::default(),
            block2: Block2Params::default(),
            block3: Block3Params::default(),
            max_delay_frames: default_max_delay_frames(),
            pick_state: PickState::default(),
        }
    }
}

fn default_max_delay_frames() -> u32 {
    30
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickState {
    #[default]
    Idle,
    Armed { target: KeyTarget },
    Pending { target: KeyTarget },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyTarget {
    Ch2,
    Fb1,
    Fb2,
    Final,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip() {
        let mut state = WaaavesState::default();
        // Mutate several fields
        state.block1.ch1_rotate = 1.23;
        state.block1.fb1_mix_amount = 0.75;
        state.block1.fb1_delay_time = 15;
        state.block2.fb2_delay_time = 8;
        state.block2.block2_input_rotate = 2.34;
        state.block3.matrix_mix_r_to_r = 0.5;
        state.block3.final_mix_amount = 0.9;
        state.max_delay_frames = 45;
        state.pick_state = PickState::Armed {
            target: KeyTarget::Ch2,
        };

        let json = serde_json::to_string(&state).unwrap();
        let restored: WaaavesState = serde_json::from_str(&json).unwrap();

        assert!((restored.block1.ch1_rotate - 1.23).abs() < 0.001);
        assert!((restored.block1.fb1_mix_amount - 0.75).abs() < 0.001);
        assert_eq!(restored.block1.fb1_delay_time, 15);
        assert_eq!(restored.block2.fb2_delay_time, 8);
        assert!((restored.block2.block2_input_rotate - 2.34).abs() < 0.001);
        assert!((restored.block3.matrix_mix_r_to_r - 0.5).abs() < 0.001);
        assert!((restored.block3.final_mix_amount - 0.9).abs() < 0.001);
        assert_eq!(restored.max_delay_frames, 45);
        assert_eq!(restored.pick_state, PickState::Idle); // serde skip resets it
    }
}
