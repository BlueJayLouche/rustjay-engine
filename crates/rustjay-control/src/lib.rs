//! rustjay-control — MIDI, OSC, and web remote control.

#![warn(missing_docs)]

pub(crate) mod midi;
pub(crate) mod osc;
pub(crate) mod web;

pub use midi::{MidiManager, MidiMapping, MidiState, LearnState};
#[cfg(feature = "mtc")]
pub use midi::mtc::MtcReceiver;
pub use osc::OscServer;
pub use web::{
    WebServer, WebConfig, WebCommand,
    InputWebCommand, OutputWebCommand, AudioWebCommand,
    ControlWebCommand, ModulationWebCommand, PresetWebCommand,
    LinkWebCommand, ProDjWebCommand,
    InputStateJson, ControlStateJson, ModulationStateJson, PresetStateJson, PresetInfo,
};
