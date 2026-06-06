//! rustjay-control — MIDI, OSC, and web remote control.

#![warn(missing_docs)]

pub(crate) mod midi;
pub(crate) mod osc;
pub(crate) mod web;

#[cfg(feature = "mtc")]
pub use midi::mtc::MtcReceiver;
pub use midi::{LearnState, MidiManager, MidiMapping, MidiState};
pub use osc::OscServer;
pub use web::{
    AudioWebCommand, ControlStateJson, ControlWebCommand, InputStateJson, InputWebCommand,
    LinkWebCommand, ModulationStateJson, ModulationWebCommand, OutputWebCommand, PresetInfo,
    PresetStateJson, PresetWebCommand, ProDjWebCommand, WebCommand, WebConfig, WebServer,
    WebServerState,
};
