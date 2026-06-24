//! Protocol message types (OSC, MSC).
//!
//! These are pure data definitions. The actual networking implementations live in
//! `qplayer-protocols`.

/// An OSC address pattern (e.g. `/qplayer/go`).
pub type OscAddress = String;

/// An MSC command type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MscCommand {
    Go,
    Stop,
    Resume,
    TimedGo,
    Load,
    Reset,
    GoOff,
}

/// A parsed MSC packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MscPacket {
    pub command: MscCommand,
    pub cue_number: String,
    pub device_id: u8,
}
