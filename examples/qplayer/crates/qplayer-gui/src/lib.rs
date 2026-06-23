//! QPlayer GUI — egui + wgpu immediate-mode interface.
//!
//! Replaces all WPF Views and ViewModels.

pub mod active_cues;
pub mod app;
pub mod cue_list;
pub mod inspector;
pub mod log_window;
pub mod logging;
pub mod transport;
pub mod waveform;

pub use app::{ActiveCueInfo, AppCommand, GuiMeterData, QPlayerApp, SharedState, SharedStateHandle, ShowMode};
