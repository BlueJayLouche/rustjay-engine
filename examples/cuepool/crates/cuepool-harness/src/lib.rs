//! CuePool test harness — drives the shipping show engine with no window, GPU,
//! audio device, or wall clock. Video uses the production FFmpeg decoder but
//! stops before presentation.

pub mod clock;
pub mod rng;
pub mod runner;
pub mod sink;

pub use runner::{HeadlessShowRunner, RunnerTrace};
