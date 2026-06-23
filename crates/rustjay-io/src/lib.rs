#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

pub(crate) mod input;
pub(crate) mod output;
pub(crate) mod texture_utils;

#[cfg(target_os = "linux")]
pub(crate) mod v4l2_devices;

#[cfg(feature = "ffmpeg")]
pub use input::ffmpeg::{detect_hap_codec, FfmpegDecoder, LoopMode, StreamDecoder, VideoFrame};
#[cfg(feature = "webcam")]
pub use input::webcam::{WebcamCapture, WebcamFrame, list_cameras};
pub use input::InputManager;
pub use input::SpoutSenderInfo;
pub use input::SyphonServerInfo;
#[cfg(feature = "ndi")]
pub use input::{NdiReceiver, list_ndi_sources};
#[cfg(target_os = "macos")]
pub use input::{SyphonInputReceiver, SyphonDiscovery};
#[cfg(target_os = "windows")]
pub use input::{SpoutDiscovery, SpoutInputReceiver};
pub use output::recorder::{Recorder, RecorderCodec};
pub use output::OutputManager;
#[cfg(target_os = "linux")]
pub use v4l2_devices::V4l2DeviceInfo;
