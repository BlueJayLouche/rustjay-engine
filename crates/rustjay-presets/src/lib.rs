//! Preset management for saving and loading engine parameter snapshots.
#![warn(missing_docs)]

pub mod presets;
pub use presets::{presets_dir_for, Preset, PresetBank};
