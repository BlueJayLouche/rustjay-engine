// `objc::msg_send!` expands to `sel!`, so the macros have to be imported even
// though every call is path-qualified. Only the webcam backend uses them.
#[cfg(all(target_os = "macos", feature = "webcam"))]
#[macro_use]
extern crate objc;

pub(crate) mod input;
pub(crate) mod output;
pub(crate) mod texture_utils;

#[cfg(target_os = "linux")]
pub(crate) mod v4l2_devices;

#[cfg(feature = "hap-encode")]
pub mod hap_encode;

#[cfg(feature = "ffmpeg")]
pub use input::ffmpeg::{detect_hap_codec, FfmpegDecoder, LoopMode, StreamDecoder, VideoFrame};
#[cfg(feature = "webcam")]
pub use input::webcam::{WebcamCapture, WebcamFrame, list_cameras};
// Without the feature the same names still exist, so a caller compiles either
// way — `list_cameras` simply finds nothing.
#[cfg(not(feature = "webcam"))]
pub use input::{WebcamFrame, list_cameras};
pub use input::InputManager;
pub use input::SpoutSenderInfo;
pub use input::SyphonServerInfo;
#[cfg(feature = "ndi")]
pub use input::{NdiReceiver, list_ndi_sources};
#[cfg(target_os = "macos")]
pub use input::{SyphonInputReceiver, SyphonDiscovery};
#[cfg(target_os = "windows")]
pub use input::{SpoutDiscovery, SpoutInputReceiver};
pub use output::recorder::{list_audio_devices, Recorder, RecorderCodec};
pub use output::OutputManager;
#[cfg(target_os = "linux")]
pub use v4l2_devices::V4l2DeviceInfo;
