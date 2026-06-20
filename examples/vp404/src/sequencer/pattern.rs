//! A pattern is a bank of 16 tracks, one per pad.

use serde::{Deserialize, Serialize};

use super::track::Track;
use crate::bank::PAD_COUNT;

/// A pattern contains one track per pad.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    pub index: usize,
    pub name: String,
    pub tracks: Vec<Track>,
}

impl Pattern {
    pub fn new(index: usize) -> Self {
        Self {
            index,
            name: format!("Pattern {:02}", index + 1),
            tracks: (0..PAD_COUNT).map(Track::new).collect(),
        }
    }

    pub fn get_track(&self, pad_index: usize) -> Option<&Track> {
        self.tracks.get(pad_index)
    }

    pub fn get_track_mut(&mut self, pad_index: usize) -> Option<&mut Track> {
        self.tracks.get_mut(pad_index)
    }

    pub fn set_length(&mut self, length: usize) {
        for track in &mut self.tracks {
            track.set_length(length);
        }
    }

    pub fn length(&self) -> usize {
        self.tracks.first().map(|t| t.length).unwrap_or(16)
    }

    pub fn clear(&mut self) {
        for track in &mut self.tracks {
            track.clear();
        }
    }

    pub fn active_pads(&self) -> Vec<usize> {
        self.tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.steps.iter().any(|s| s.active))
            .map(|(i, _)| i)
            .collect()
    }
}

impl Default for Pattern {
    fn default() -> Self {
        Self::new(0)
    }
}
