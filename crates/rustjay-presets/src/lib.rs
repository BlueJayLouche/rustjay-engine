//! Preset management for saving and loading engine parameter snapshots.
#![warn(missing_docs)]

pub mod presets;
pub use presets::{Preset, PresetBank, presets_dir_for};
