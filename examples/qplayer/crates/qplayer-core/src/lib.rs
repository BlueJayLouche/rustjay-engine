//! QPlayer Core — Domain models, serialization, and show file migration.
//!
//! This crate is pure logic: no I/O, no threading primitives, no OS dependencies.
//! It defines the data types that cross all layer boundaries.

pub mod colour;
pub mod cue;
pub mod eq;
pub mod peakfile;
pub mod protocol;
pub mod showfile;
pub mod timespan;

pub use colour::SerializedColour;
pub use cue::{Cue, CueBase, DummyCue, GroupCue, SoundCue, StopCue, TimeCodeCue, VolumeCue};
pub use cue::{FadeType, LoopMode, StopMode, TriggerMode};
pub use eq::{EQBand, EQBandShape, EQFilter, EQFilterOrder, EQSettings};
pub use peakfile::{PeakFile, PeakFileError, PeakFileReader, PeakFileWriter};
pub use showfile::{AudioLimiterSettings, AudioOutputDriver, RemoteNode, ShowFile, ShowSettings};
pub use timespan::Timespan;
