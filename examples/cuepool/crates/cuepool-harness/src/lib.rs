//! CuePool test harness — drives the show-critical crates with no window, no
//! audio device and no FFmpeg, so CI and unattended agents can verify behaviour.
//!
//! Deliberately excludes cuepool-video and cuepool-gui: they need FFmpeg and
//! wgpu, and cannot link headless.

pub mod clock;
pub mod rng;
pub mod sink;
