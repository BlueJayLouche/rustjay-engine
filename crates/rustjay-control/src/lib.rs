//! rustjay-control — MIDI, OSC, and web remote control.

#![warn(missing_docs)]

pub(crate) mod midi;
pub(crate) mod osc;
pub(crate) mod web;

pub use midi::{MidiManager, MidiState};
pub use osc::OscServer;
pub use web::{WebServer, WebConfig, WebCommand};
