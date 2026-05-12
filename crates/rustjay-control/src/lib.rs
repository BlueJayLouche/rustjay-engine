pub mod midi;
pub mod osc;
pub mod web;

pub use midi::{MidiManager, MidiState};
pub use osc::OscServer;
pub use web::{WebServer, WebConfig, WebCommand};
