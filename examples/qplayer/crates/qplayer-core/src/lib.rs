//! QPlayer Core — Domain models, serialization, and show file migration.
//!
//! This crate is pure logic: no I/O, no OS dependencies. It defines the data types
//! that cross all layer boundaries (plus one shared sync helper, [`sync::LockExt`]).

pub mod colour;
pub mod cue;
pub mod eq;
pub mod projection;
pub mod showfile;
pub mod sync;
pub mod timespan;

pub use colour::SerializedColour;
pub use cue::{AudioRouting, Crosspoint, Cue, CueBase, DummyCue, GotoCue, GroupCue, ImageCue, OscCue, SoundCue, StopCue, TextCue, TimeCodeCue, VideoCue, VolumeCue};
pub use cue::{ClockMode, CueTriggers, FadeType, HotkeyTrigger, LoopMode, MidiTrigger, MidiTriggerKind, RepeatMode, StopMode, TimecodeTrigger, TriggerMode, WallClockTrigger};
pub use eq::{EQBand, EQBandShape, EQFilter, EQFilterOrder, EQSettings};
pub use projection::{CanvasFit, EdgeBlend, EdgeBlendEdge, MonitorId, ProjectionConfig, ProjectorOutput, resolve_monitor_assignment};
pub use showfile::{AudioLimiterSettings, AudioOutputDriver, RemoteNode, ShowFile, ShowSettings};
pub use sync::LockExt;
pub use timespan::Timespan;
