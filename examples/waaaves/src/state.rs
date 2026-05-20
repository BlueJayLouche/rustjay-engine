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
        let state = WaaavesState::default();
        let json = serde_json::to_string(&state).unwrap();
        let restored: WaaavesState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.max_delay_frames, state.max_delay_frames);
        assert_eq!(restored.pick_state, PickState::Idle); // serde skip resets it
    }
}
