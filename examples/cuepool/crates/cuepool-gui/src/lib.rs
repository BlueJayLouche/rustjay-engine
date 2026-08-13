//! CuePool GUI — egui + wgpu immediate-mode interface.
//!
//! Replaces all WPF Views and ViewModels.

pub mod active_cues;
pub mod app;
pub mod cue_list;
pub mod inspector;
pub mod log_window;
pub mod logging;
pub mod lighting_panel;
pub mod preview;
pub mod recorder_panel;
pub mod status_panel;
pub mod take_editor;
pub mod projection_panel;
pub mod transport;
pub mod waveform;

pub use app::{ActiveCueInfo, AppCommand, CuePoolApp, DecodeTiming, Diagnostics, GuiMeterData, OutputDiagnostics, SharedState, SharedStateHandle, ShowMode, VideoDiagnostics};
