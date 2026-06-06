//! rustjay-io — video input and output management.
//!
//! Handles capture from webcams, NDI, Syphon, Spout, and V4L2,
//! and output streaming via the same protocols.

#![warn(missing_docs)]

#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

pub(crate) mod input;
pub(crate) mod ndi_runtime;
pub(crate) mod output;
pub(crate) mod texture_utils;

#[cfg(target_os = "linux")]
pub(crate) mod v4l2_devices;

#[cfg(feature = "ffmpeg")]
pub use input::ffmpeg::{FfmpegDecoder, LoopMode, StreamDecoder, VideoFrame};
#[cfg(feature = "webcam")]
pub use input::webcam::{WebcamCapture, WebcamFrame};
pub use input::InputManager;
pub use input::SpoutSenderInfo;
pub use input::SyphonServerInfo;
pub use output::recorder::{Recorder, RecorderCodec};
pub use output::OutputManager;
#[cfg(target_os = "linux")]
pub use v4l2_devices::V4l2DeviceInfo;
