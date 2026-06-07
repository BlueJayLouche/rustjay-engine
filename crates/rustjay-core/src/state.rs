//! Engine state and command enums.
//!
//! [`EngineState`] is the central mutable state that the engine manages and
//! that app plugins read from (via [`EffectPlugin::build_uniforms`](crate::EffectPlugin::build_uniforms)).


use crate::modulation::ModulationEngine;
use crate::params::{ParamCategory, ParameterDescriptor};
use crate::routing::AudioRoutingState;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// ── Command enums ──────────────────────────────────────────────────────────

/// Commands sent to the input subsystem.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum InputCommand {
    /// No-op.
    #[default]
    None,
    /// Start capturing from a webcam.
    StartWebcam {
        /// Device index in the discovered list.
        device_index: usize,
        /// Requested capture width.
        width: u32,
        /// Requested capture height.
        height: u32,
        /// Requested capture frame rate.
        fps: u32,
    },
    /// Start receiving from an NDI source (NDI feature only).
    #[cfg(feature = "ndi")]
    StartNdi {
        /// NDI source name to connect to.
        source_name: String,
    },
    /// Start receiving from a Syphon server (macOS only).
    #[cfg(target_os = "macos")]
    StartSyphon {
        /// Syphon server name.
        server_name: String,
        /// Syphon server UUID.
        server_uuid: String,
    },
    /// Start receiving from a Spout sender (Windows only).
    #[cfg(target_os = "windows")]
    StartSpout {
        /// Spout sender name.
        sender_name: String,
    },
    /// Start capturing from a V4L2 device (Linux only).
    #[cfg(target_os = "linux")]
    StartV4l2 {
        /// V4L2 device path (e.g. `/dev/video0`).
        device_path: String,
    },
    /// Stop the current input.
    StopInput,
    /// Refresh the list of available input devices.
    RefreshDevices,
}

/// Target codec for disk recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderCodec {
    /// H.264 / AVC.
    H264,
    /// H.265 / HEVC.
    H265,
    /// AV1.
    AV1,
    /// Apple ProRes 422.
    ProRes422,
}

/// Commands sent to the output subsystem.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum OutputCommand {
    /// No-op.
    #[default]
    None,
    /// Start NDI output streaming (NDI feature only).
    #[cfg(feature = "ndi")]
    StartNdi,
    /// Stop NDI output streaming (NDI feature only).
    #[cfg(feature = "ndi")]
    StopNdi,
    /// Start Syphon output server (macOS only).
    #[cfg(target_os = "macos")]
    StartSyphon,
    /// Stop Syphon output server (macOS only).
    #[cfg(target_os = "macos")]
    StopSyphon,
    /// Start Spout output sender (Windows only).
    #[cfg(target_os = "windows")]
    StartSpout {
        /// Spout sender name.
        sender_name: String,
    },
    /// Stop Spout output sender (Windows only).
    #[cfg(target_os = "windows")]
    StopSpout,
    /// Start V4L2 loopback output (Linux only).
    #[cfg(target_os = "linux")]
    StartV4l2 {
        /// V4L2 loopback device path.
        device_path: String,
    },
    /// Stop V4L2 loopback output (Linux only).
    #[cfg(target_os = "linux")]
    StopV4l2,
    /// Re-initialize outputs after a resolution change.
    ResizeOutput,
    /// Start disk recording.
    StartRecording {
        /// Output file path.
        path: String,
        /// Target codec.
        codec: RecorderCodec,
    },
    /// Stop disk recording.
    StopRecording,
}

/// Commands sent to the audio subsystem.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum AudioCommand {
    /// No-op.
    #[default]
    None,
    /// Start audio capture and analysis.
    Start,
    /// Stop audio capture.
    Stop,
    /// Refresh the list of audio devices.
    RefreshDevices,
    /// Select an audio input device by name.
    SelectDevice(String),
    /// Change the FFT analysis window size.
    SetFftSize(usize),
}

/// The type of MIDI message used in a CC/Note/AT mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MidiMsgKind {
    /// Control Change — continuous knobs, faders, pedals.
    Cc,
    /// Note On / Note Off — pads, keys. Note Off drives the parameter to its minimum.
    Note,
    /// Channel Aftertouch — mono pressure from a keyboard or pad controller.
    Aftertouch,
}

/// A serializable snapshot of one MIDI mapping, used in presets and the engine→GUI sync.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiMappingSnapshot {
    /// Human-readable parameter name.
    pub name: String,
    /// Parameter path (e.g. `"color/hue_shift"`).
    pub param_path: String,
    /// Message type.
    pub kind: MidiMsgKind,
    /// CC number, note number, or 0 for channel aftertouch.
    pub selector: u8,
    /// MIDI channel (0–15).
    pub channel: u8,
    /// Minimum output value.
    pub min_value: f32,
    /// Maximum output value.
    pub max_value: f32,
}

/// Commands sent to the MIDI subsystem.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum MidiCommand {
    /// No-op.
    #[default]
    None,
    /// Refresh the list of MIDI devices.
    RefreshDevices,
    /// Select a MIDI input device by name.
    SelectDevice(String),
    /// Enter CC-learn mode for the given parameter.
    StartLearn {
        /// Hierarchical path used to identify the parameter.
        param_path: String,
        /// Human-readable parameter name.
        param_name: String,
        /// Parameter minimum value (used to scale the CC output range).
        min: f32,
        /// Parameter maximum value (used to scale the CC output range).
        max: f32,
    },
    /// Cancel CC-learn mode.
    CancelLearn,
    /// Clear all CC mappings.
    ClearMappings,
    /// Disconnect the current MIDI device.
    Disconnect,
    /// Replace all mappings (used when loading a preset).
    RestoreMappings(Vec<MidiMappingSnapshot>),
}

/// Commands sent to the modulation subsystem.
#[derive(Debug, Clone, Default)]
pub enum ModulationCommand {
    /// No-op.
    #[default]
    None,
    /// Add a modulation source.
    AddSource(crate::modulation::ModulationSource),
    /// Add a modulation source with a specific UUID.
    AddSourceWithUuid {
        /// Stable UUID for the source.
        uuid: String,
        /// The modulation source to add.
        source: crate::modulation::ModulationSource,
    },
    /// Remove a modulation source by UUID.
    RemoveSource(String),
    /// Assign a source to a parameter.
    Assign {
        /// Target parameter id.
        param: String,
        /// Source UUID.
        source_id: String,
        /// Modulation amount.
        amount: f32,
        /// Optional component index (for color params).
        component: Option<usize>,
    },
    /// Assign mod-on-mod (a modulator targets another source's param).
    AssignModOnMod {
        /// Target source UUID.
        target_uuid: String,
        /// Target parameter within the source.
        param: String,
        /// Modulator source UUID.
        modulator_uuid: String,
        /// Modulation amount.
        amount: f32,
    },
    /// Clear all assignments for a parameter.
    ClearAssignments(String),
    /// Trigger an ADSR envelope.
    TriggerAdsr(String),
    /// Release an ADSR envelope.
    ReleaseAdsr(String),
    /// Replace the entire modulation engine state (preset restore).
    RestoreEngine(crate::modulation::ModulationEngine),
}

/// Commands sent to the OSC subsystem.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum OscCommand {
    /// No-op.
    #[default]
    None,
    /// Start the OSC server.
    Start,
    /// Stop the OSC server.
    Stop,
    /// Change the OSC listen port.
    SetPort(u16),
    /// Re-scan for auto-generated OSC addresses.
    RefreshAddresses,
}

/// Commands sent to the preset subsystem.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum PresetCommand {
    /// No-op.
    #[default]
    None,
    /// Save the current state as a new preset.
    Save {
        /// Preset name.
        name: String,
    },
    /// Load a preset by index.
    Load(usize),
    /// Delete a preset by index.
    Delete(usize),
    /// Apply the preset assigned to a quick slot.
    ApplySlot(usize),
    /// Assign a preset to a quick slot.
    AssignSlot {
        /// Index of the preset to assign.
        preset_index: usize,
        /// Quick slot number (1–8).
        slot: usize,
    },
    /// Refresh the preset list from disk.
    Refresh,
}

/// Commands sent to the web remote subsystem.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum WebCommand {
    /// No-op.
    #[default]
    None,
    /// Start the web remote server.
    Start,
    /// Stop the web remote server.
    Stop,
    /// Change the web server port.
    SetPort(u16),
    /// Enable or disable LAN trust mode (skip token auth for local network clients).
    SetLanTrust(bool),
}

/// Commands sent to the Ableton Link subsystem.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum LinkCommand {
    /// No-op.
    #[default]
    None,
    /// Enable Link session participation.
    Enable,
    /// Disable Link session participation.
    Disable,
    /// Change the musical quantum (e.g. 4.0 for a 4/4 bar).
    SetQuantum(f64),
}

/// Commands sent to the ProDJ Link subsystem.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ProDjCommand {
    /// No-op.
    #[default]
    None,
    /// Start listening for ProDJ Link devices.
    Start,
    /// Stop listening and clear discovered devices.
    Stop,
}

// ── Input type ─────────────────────────────────────────────────────────────

/// Discriminant for the active video input source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum InputType {
    /// No input active.
    #[default]
    None,
    /// Webcam / capture device.
    Webcam,
    /// NDI network source (NDI feature only).
    #[cfg(feature = "ndi")]
    Ndi,
    /// Syphon frame receiver (macOS only).
    #[cfg(target_os = "macos")]
    Syphon,
    /// Spout frame receiver (Windows only).
    #[cfg(target_os = "windows")]
    Spout,
    /// V4L2 capture device (Linux only).
    #[cfg(target_os = "linux")]
    V4l2,
}

impl InputType {
    /// Human-readable name for display in the UI.
    pub fn name(&self) -> &'static str {
        match self {
            InputType::None => "None",
            InputType::Webcam => "Webcam",
            #[cfg(feature = "ndi")]
            InputType::Ndi => "NDI",
            #[cfg(target_os = "macos")]
            InputType::Syphon => "Syphon",
            #[cfg(target_os = "windows")]
            InputType::Spout => "Spout",
            #[cfg(target_os = "linux")]
            InputType::V4l2 => "V4L2",
        }
    }
}

// ── Sub-states ─────────────────────────────────────────────────────────────

/// Discovered video capture device info.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InputDeviceInfo {
    /// Human-readable device name.
    pub name: String,
    /// Device path (e.g. `/dev/video0`).
    pub path: String,
    /// Device index.
    pub index: usize,
}

/// Live state of the video input device.
#[derive(Debug, Clone, Default)]
pub struct InputState {
    /// Active input source type.
    pub input_type: InputType,
    /// Name or identifier of the current source.
    pub source_name: String,
    /// Whether the input is currently streaming.
    pub is_active: bool,
    /// Capture width in pixels.
    pub width: u32,
    /// Capture height in pixels.
    pub height: u32,
    /// Capture frame rate (may be approximate).
    pub fps: f32,
    /// Numeric device index of the active webcam (None if not a webcam or not started).
    pub device_index: Option<usize>,
    /// Discovered video capture devices.
    pub available_devices: Vec<InputDeviceInfo>,
}

/// HSB (Hue / Saturation / Brightness) colour adjustment parameters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HsbParams {
    /// Hue shift in degrees (-180 to 180).
    pub hue_shift: f32,
    /// Saturation multiplier (0 to 2).
    pub saturation: f32,
    /// Brightness multiplier (0 to 2).
    pub brightness: f32,
}

impl Default for HsbParams {
    fn default() -> Self {
        Self {
            hue_shift: 0.0,
            saturation: 1.0,
            brightness: 1.0,
        }
    }
}

impl HsbParams {
    /// Reset to defaults (no shift, unity saturation and brightness).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Selects which external source drives the engine's tempo and beat phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SyncSource {
    /// Audio beat detection and tap tempo. Default.
    #[default]
    Audio,
    /// Ableton Link — joins a shared Link session.
    AbletonLink,
    /// ProDJ Link — reads tempo from Pioneer CDJ/XDJ gear.
    ProDj,
}

impl SyncSource {
    /// Human-readable label for UI display.
    pub fn name(self) -> &'static str {
        match self {
            SyncSource::Audio => "Audio / Tap Tempo",
            SyncSource::AbletonLink => "Ableton Link",
            SyncSource::ProDj => "ProDJ Link",
        }
    }
}

/// Live state of Ableton Link sync.
#[derive(Debug, Clone)]
pub struct LinkState {
    /// Whether Link session participation is active.
    pub enabled: bool,
    /// Number of peers in the current Link session.
    pub num_peers: usize,
    /// Current tempo from Link (BPM).
    pub bpm: f32,
    /// Current position within a beat cycle (0–1).
    pub beat_phase: f32,
    /// Musical quantum used for beat/phase calculations.
    pub quantum: f64,
    /// Whether the Link session is currently playing.
    pub is_playing: bool,
}

impl Default for LinkState {
    fn default() -> Self {
        Self {
            enabled: false,
            num_peers: 0,
            bpm: 0.0,
            beat_phase: 0.0,
            quantum: 4.0,
            is_playing: false,
        }
    }
}

/// Discovered CDJ/XDJ device on a ProDJ Link network.
#[derive(Debug, Clone)]
pub struct CdjDevice {
    /// Device ID assigned by the ProDJ Link network.
    pub device_id: u32,
    /// Human-readable device name.
    pub name: String,
    /// Whether the deck is currently playing.
    pub is_playing: bool,
    /// Whether this deck is the current tempo master.
    pub is_master: bool,
    /// Current BPM reported by the deck, if available.
    pub bpm: Option<f32>,
}

/// SMPTE frame rate reported by an MTC source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MtcFrameRate {
    /// 24 frames per second.
    Fps24,
    /// 25 frames per second.
    #[default]
    Fps25,
    /// 29.97 frames per second, drop-frame.
    Fps2997Drop,
    /// 30 frames per second.
    Fps30,
}

impl MtcFrameRate {
    /// Nominal frames per second as a float.
    pub fn fps(self) -> f32 {
        match self {
            MtcFrameRate::Fps24 => 24.0,
            MtcFrameRate::Fps25 => 25.0,
            MtcFrameRate::Fps2997Drop => 29.97,
            MtcFrameRate::Fps30 => 30.0,
        }
    }

    /// Short human-readable label.
    pub fn name(self) -> &'static str {
        match self {
            MtcFrameRate::Fps24 => "24fps",
            MtcFrameRate::Fps25 => "25fps",
            MtcFrameRate::Fps2997Drop => "29.97fps DF",
            MtcFrameRate::Fps30 => "30fps",
        }
    }
}

/// A SMPTE HH:MM:SS:FF timecode position.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SmpteTime {
    /// Hours component (0–23).
    pub hours: u8,
    /// Minutes component (0–59).
    pub minutes: u8,
    /// Seconds component (0–59).
    pub seconds: u8,
    /// Frames component (0 to fps-1).
    pub frames: u8,
    /// Frame rate reported by the MTC source.
    pub frame_rate: MtcFrameRate,
}

impl SmpteTime {
    /// Timecode as fractional elapsed seconds.
    pub fn as_seconds_f64(self) -> f64 {
        let fps = self.frame_rate.fps() as f64;
        self.hours as f64 * 3600.0
            + self.minutes as f64 * 60.0
            + self.seconds as f64
            + self.frames as f64 / fps
    }
}

impl std::fmt::Display for SmpteTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:02}:{:02}:{:02}:{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

/// Live state of MIDI Timecode (MTC) receive.
#[derive(Debug, Clone, Default)]
pub struct MtcState {
    /// `true` once any MTC message has been received on any MIDI port.
    pub running: bool,
    /// `true` while quarter-frame messages are arriving (transport playing/shuttling).
    pub playing: bool,
    /// Most recently assembled SMPTE timecode position.
    pub position: SmpteTime,
    /// Name of the MIDI port currently sending MTC (empty string if none yet).
    pub source_device: String,
}

/// Live state of ProDJ Link sync.
#[derive(Debug, Clone, Default)]
pub struct ProDjState {
    /// Whether ProDJ Link discovery is active.
    pub enabled: bool,
    /// Discovered CDJ/XDJ devices.
    pub devices: Vec<CdjDevice>,
    /// Current master BPM (0.0 if no master).
    pub master_bpm: f32,
    /// Current master beat phase (0–1).
    pub master_beat_phase: f32,
    /// Artist of the current master track.
    pub current_track_artist: String,
    /// Title of the current master track.
    pub current_track_title: String,
}

/// Live state of the audio analysis subsystem.
#[derive(Debug, Clone)]
pub struct AudioState {
    /// Per-band FFT magnitudes (8 bands, 0–1).
    pub fft: [f32; 8],
    /// Overall volume level (0–1).
    pub volume: f32,
    /// True if a beat was detected this frame.
    pub beat: bool,
    /// Estimated beats-per-minute.
    pub bpm: f32,
    /// Current position within a beat cycle (0–1).
    pub beat_phase: f32,
    /// Whether audio analysis is active.
    pub enabled: bool,
    /// Input gain applied before FFT.
    pub amplitude: f32,
    /// Smoothing factor for FFT output (0–1).
    pub smoothing: f32,
    /// Name of the selected audio device, if any.
    pub selected_device: Option<String>,
    /// Names of all discovered audio devices.
    pub available_devices: Vec<String>,
    /// Whether automatic peak normalisation is enabled.
    pub normalize: bool,
    /// Whether pink-noise compensation shaping is enabled.
    pub pink_noise_shaping: bool,
    /// FFT window size (1024, 2048, 4096, or 8192).
    pub fft_size: usize,
    /// Recent tap-tempo timestamps (seconds since epoch).
    pub tap_times: Vec<f64>,
    /// Timestamp of the most recent tap.
    pub last_tap_time: f64,
    /// Human-readable tap-tempo feedback message.
    pub tap_tempo_info: String,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            fft: [0.0; 8],
            volume: 0.0,
            beat: false,
            bpm: 120.0,
            beat_phase: 0.0,
            enabled: true,
            amplitude: 1.0,
            smoothing: 0.5,
            selected_device: None,
            available_devices: Vec::new(),
            normalize: true,
            pink_noise_shaping: false,
            fft_size: 2048,
            tap_times: Vec::new(),
            last_tap_time: 0.0,
            tap_tempo_info: "Tap to set tempo".to_string(),
        }
    }
}

/// NDI output configuration (available when the `ndi` feature is enabled).
#[cfg(feature = "ndi")]
#[derive(Debug, Clone, Default)]
pub struct NdiOutputState {
    /// Stream name advertised on the network.
    pub stream_name: String,
    /// Whether the NDI output is currently streaming.
    pub is_active: bool,
    /// Whether to include an alpha channel.
    pub include_alpha: bool,
}

/// Syphon output configuration (macOS only).
#[derive(Debug, Clone, Default)]
pub struct SyphonOutputState {
    /// Syphon server name.
    pub server_name: String,
    /// Whether the Syphon output is active.
    pub enabled: bool,
}

/// Spout output configuration (Windows only).
#[derive(Debug, Clone, Default)]
pub struct SpoutOutputState {
    /// Spout sender name.
    pub sender_name: String,
    /// Whether the Spout output is active.
    pub enabled: bool,
}

/// V4L2 loopback output configuration (Linux only).
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Default)]
pub struct V4l2OutputState {
    /// V4L2 loopback device path.
    pub device_path: String,
    /// Whether the V4L2 output is active.
    pub enabled: bool,
}

/// Internal rendering and input resolution.
#[derive(Debug, Clone)]
pub struct ResolutionState {
    /// Internal render target width.
    pub internal_width: u32,
    /// Internal render target height.
    pub internal_height: u32,
    /// Width of the active input texture.
    pub input_width: u32,
    /// Height of the active input texture.
    pub input_height: u32,
}

impl Default for ResolutionState {
    fn default() -> Self {
        Self {
            internal_width: 1920,
            internal_height: 1080,
            input_width: 1920,
            input_height: 1080,
        }
    }
}

/// Frame-rate and frame-time metrics.
#[derive(Debug, Clone, Copy, Default)]
pub struct PerformanceMetrics {
    /// Current frames per second.
    pub fps: f32,
    /// Average frame time in milliseconds.
    pub frame_time_ms: f32,
    /// Global CPU usage percentage (populated externally via sysinfo).
    /// Zero when the sysmon feature is disabled or not yet polled.
    pub cpu_percent: f32,
    /// Used memory in megabytes (populated externally via sysinfo).
    /// Zero when the sysmon feature is disabled or not yet polled.
    pub mem_used_mb: u64,
    /// Total memory in megabytes (populated externally via sysinfo).
    /// Zero when the sysmon feature is disabled or not yet polled.
    pub mem_total_mb: u64,
}

/// Severity level of a notification toast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationLevel {
    /// Neutral informational message.
    Info,
    /// Successful action completed.
    Success,
    /// Warning that may need attention.
    Warning,
    /// Error or failure.
    Error,
}

/// One transient toast notification.
#[derive(Debug, Clone)]
pub struct Notification {
    /// Stable identifier for egui keys.
    pub id: u64,
    /// Human-readable message.
    pub message: String,
    /// Visual severity.
    pub level: NotificationLevel,
    /// When the toast should be removed.
    pub expires_at: std::time::Instant,
}

/// Built-in tabs rendered by the engine's control GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GuiTab {
    /// Video input selection.
    #[default]
    Input,
    /// Colour / HSB adjustment.
    Color,
    /// Motion / spatial effect parameters.
    Motion,
    /// Audio analysis and routing.
    Audio,
    /// Video output configuration.
    Output,
    /// Preset save / load.
    Presets,
    /// MIDI device and mapping.
    Midi,
    /// OSC server settings.
    Osc,
    /// Web remote settings.
    Web,
    /// General application settings.
    Settings,
    /// Tempo sync (Ableton Link + ProDJ).
    Sync,
    /// Unified modulation control (LFO, ADSR, step sequencer, audio band).
    Modulation,
}

impl GuiTab {
    /// Human-readable tab label.
    pub fn name(&self) -> &'static str {
        match self {
            GuiTab::Input => "Input",
            GuiTab::Color => "Color",
            GuiTab::Motion => "Motion",
            GuiTab::Audio => "Audio",
            GuiTab::Output => "Output",
            GuiTab::Presets => "Presets",
            GuiTab::Midi => "MIDI",
            GuiTab::Osc => "OSC",
            GuiTab::Web => "Web",
            GuiTab::Settings => "Settings",
            GuiTab::Sync => "Sync",
            GuiTab::Modulation => "Modulation",
        }
    }

    /// All standard built-in tabs in their default order.
    pub fn all() -> &'static [GuiTab] {
        &[
            GuiTab::Input,
            GuiTab::Color,
            GuiTab::Motion,
            GuiTab::Audio,
            GuiTab::Output,
            GuiTab::Presets,
            GuiTab::Midi,
            GuiTab::Osc,
            GuiTab::Web,
            GuiTab::Modulation,
            // Settings lives in View > Preferences (menu bar), not a tab.
            // Sync is folded into the Audio tab.
            // Both variants are kept for serialization / hidden_tabs filtering.
        ]
    }
}

// ── EngineState ────────────────────────────────────────────────────────────

/// Type alias for the param resolver closure.
pub type ParamResolverFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// App-specific parameter resolver: maps hierarchical control paths to flat
/// engine parameter ids. Stored in [`EngineState::param_resolver`].
#[derive(Clone)]
pub struct ParamResolver(pub ParamResolverFn);

impl std::fmt::Debug for ParamResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParamResolver").finish_non_exhaustive()
    }
}

impl ParamResolver {
    /// Resolve a hierarchical parameter path to a flat engine id.
    pub fn resolve(&self, path: &str) -> Option<String> {
        (self.0)(path)
    }
}

/// Central mutable state managed by the engine.
///
/// App plugins receive an `&EngineState` in
/// [`EffectPlugin::build_uniforms`](crate::EffectPlugin::build_uniforms) so they
/// can react to audio, LFO, and input data.
#[derive(Debug)]
pub struct EngineState {
    /// Whether the output window is fullscreen.
    pub output_fullscreen: bool,
    /// Output window width in pixels.
    pub output_width: u32,
    /// Output window height in pixels.
    pub output_height: u32,

    /// Current video input state (slot 1).
    pub input: InputState,
    /// Pending input command (slot 1).
    pub input_command: InputCommand,
    /// Second video input state (slot 2).
    pub second_input: InputState,
    /// Pending command for the second input.
    pub second_input_command: InputCommand,
    /// Shared texture view for the second input (None if no active source).
    pub second_input_view: Option<Arc<wgpu::TextureView>>,
    /// Shared sampler for the second input.
    pub second_input_sampler: Option<Arc<wgpu::Sampler>>,
    /// Generation counter for the second input texture, bumped on reallocation.
    pub second_input_generation: u64,
    /// UV coordinate requested for GPU pixel readback (set by GUI on pick-click).
    pub pick_request: Option<[f32; 2]>,
    /// RGB result of the most recent GPU readback (cleared after GUI consumes it).
    pub picked_color: Option<[f32; 3]>,
    /// Whether pixel-pick is armed (used by preview window to show crosshair).
    pub pixel_pick_armed: bool,

    /// HSB colour parameters.
    pub hsb_params: HsbParams,
    /// Base HSB colour parameters (before LFO modulation).
    pub hsb_param_bases: HsbParams,
    /// Whether HSB colour adjustment is enabled.
    pub color_enabled: bool,

    /// Audio analysis state.
    pub audio: AudioState,
    /// Pending audio command.
    pub audio_command: AudioCommand,
    /// Audio-to-parameter routing matrix.
    pub audio_routing: AudioRoutingState,

    /// Unified modulation engine (single source of truth for LFO, ADSR, audio band, step seq).
    pub modulation: Arc<Mutex<ModulationEngine>>,
    /// Pre-computed modulation offsets for each param id, updated once per frame after
    /// `ModulationEngine::update()`. `get_param()` reads this without locking.
    /// Stored as a `Vec` (not `HashMap`) because the number of modulated params is
    /// small (< 10 typical); linear scan is faster and avoids per-frame HashMap allocation.
    pub modulation_offsets: Vec<(String, f32)>,

    /// Flat list of plugin-declared parameter ids; updated on plugin load/reload.
    /// Used by the Modulation tab (M5.2) to populate the target picker.
    pub registered_param_ids: Vec<String>,


    /// NDI output state (NDI feature only).
    #[cfg(feature = "ndi")]
    pub ndi_output: NdiOutputState,
    /// Pending output command.
    pub output_command: OutputCommand,
    /// Whether disk recording is currently active.
    pub recording_active: bool,

    /// Syphon output state (macOS only).
    #[cfg(target_os = "macos")]
    pub syphon_output: SyphonOutputState,

    /// Spout output state (Windows only).
    #[cfg(target_os = "windows")]
    pub spout_output: SpoutOutputState,

    /// V4L2 output state (Linux only).
    #[cfg(target_os = "linux")]
    pub v4l2_output: V4l2OutputState,

    /// Rendering resolution.
    pub resolution: ResolutionState,
    /// Performance metrics.
    pub performance: Mutex<PerformanceMetrics>,

    /// If set, the engine will start this webcam device on the first frame
    /// after the InputManager is ready.  Cleared once the command is issued.
    pub startup_webcam_device: Option<usize>,
    /// Whether preview windows are shown.
    pub show_preview: bool,
    /// Target render frame rate in frames per second.
    pub target_fps: u32,
    /// UI scale factor.
    pub ui_scale: f32,
    /// Currently selected GUI tab.
    pub current_tab: GuiTab,

    /// Pending MIDI command.
    pub midi_command: MidiCommand,
    /// Pending modulation command.
    pub modulation_command: ModulationCommand,
    /// Available MIDI input devices (populated after RefreshDevices).
    pub midi_available_devices: Vec<String>,
    /// Currently connected MIDI device, if any.
    pub midi_selected_device: Option<String>,
    /// Whether a MIDI device is currently connected.
    pub midi_enabled: bool,
    /// Whether MIDI learn mode is active (waiting for a CC to arrive).
    pub midi_learn_active: bool,
    /// Human-readable name of the parameter currently being learned.
    pub midi_learning_param_name: Option<String>,
    /// Active mappings, synced each frame from MidiState (includes min/max for preset round-trip).
    pub midi_mappings: Vec<MidiMappingSnapshot>,
    /// Pending OSC command.
    pub osc_command: OscCommand,
    /// Whether the OSC server is running.
    pub osc_enabled: bool,
    /// OSC server listen host.
    pub osc_host: String,
    /// OSC server listen port.
    pub osc_port: u16,
    /// Recent OSC messages received (address, normalized value, timestamp).
    pub osc_message_log: Vec<(String, f32, f64)>,

    /// Pending preset command.
    pub preset_command: PresetCommand,
    /// Names of all saved presets.
    pub preset_names: Vec<String>,
    /// Names assigned to quick slots 1–8.
    pub preset_quick_slot_names: [Option<String>; 8],

    /// Set to `true` when settings should be persisted on exit.
    pub save_settings_requested: bool,
    /// Whether background input device discovery is running.
    pub input_discovering: bool,

    /// Pending web server command.
    pub web_command: WebCommand,
    /// Whether the web remote server is running.
    pub web_enabled: bool,
    /// Web server listen host.
    pub web_host: String,
    /// Web server listen port.
    pub web_port: u16,
    /// Web server app name path segment.
    pub web_app_name: String,
    /// Bearer token for the web remote (populated once the server starts).
    pub web_token: String,
    /// Full access URL including token query param (populated once the server starts).
    pub web_full_url: String,
    /// When true, clients on the same LAN subnet connect without a token.
    pub web_lan_trust: bool,

    /// Ableton Link sync state.
    pub link: LinkState,
    /// Pending Link command.
    pub link_command: LinkCommand,

    /// ProDJ Link sync state.
    pub prodj: ProDjState,
    /// Pending ProDJ command.
    pub prodj_command: ProDjCommand,

    /// MIDI Timecode (MTC) receive state.
    pub mtc: MtcState,

    /// User-selected tempo sync source (drives LFOs and beat-phase).
    pub sync_source: SyncSource,

    // ── Effect-declared parameters ───────────────────────────────────────────
    /// Effect-declared parameter descriptors (populated at init).
    /// Wrapped in Arc so cloning is a pointer copy.
    pub param_descriptors: Arc<Vec<ParameterDescriptor>>,

    /// Base values of effect parameters (set by user UI, OSC, MIDI, Web).
    /// Aligned 1:1 with `param_descriptors` — indexed by descriptor position.
    pub custom_param_bases: Vec<f32>,

    /// Modulated values of effect parameters (base + LFO + audio routing).
    /// Aligned 1:1 with `param_descriptors` — indexed by descriptor position.
    /// Effects read these in `build_uniforms`.
    pub custom_params: Vec<f32>,

    /// Pre-computed OSC address strings (`"/category/id"`) for each effect parameter.
    /// Populated once at init alongside `param_descriptors`; avoids per-frame `format!()` calls.
    pub param_osc_addresses: Vec<String>,

    /// Tabs hidden by the active effect (populated at init from `EffectPlugin::hidden_tabs`).
    pub hidden_tabs: Vec<GuiTab>,

    /// Optional prefix applied temporarily by nested effect wrappers (e.g.
    /// [`EffectNode`](rustjay_render::EffectNode)) so that a reusable effect
    /// can look up its bare parameter IDs while the engine stores them under
    /// a fully-qualified name like `ch_a_red`.
    pub param_lookup_prefix: RefCell<Option<String>>,

    /// Optional callback for resolving hierarchical parameter paths to flat
    /// engine ids. Used by app-specific param routers (e.g. Varda's
    /// `deck/<uuid>/param/<name>` → `ch_<uuid>_deck_<uuid>_<name>`) without
    /// forking the core parameter system.
    pub param_resolver: Option<ParamResolver>,

    /// Optional app-published API snapshot, stored as opaque JSON so the
    /// generic engine does not need to know the concrete app schema. Apps
    /// (e.g. `examples/varda`) publish their structure/state here; the API
    /// layer serves it verbatim via `GET /api/app/state` and includes it in
    /// the WebSocket delta stream.
    pub app_state: Arc<std::sync::Mutex<Option<serde_json::Value>>>,

    /// Transient toast notifications posted by the app or engine.
    /// Rendered globally by the control GUI and expired automatically.
    pub notifications: Arc<Mutex<Vec<Notification>>>,
    /// Monotonically increasing ID for notification egui keys.
    pub next_notification_id: AtomicU64,

    /// Opaque handle to the engine's projection subsystem, when the
    /// `projection` feature is enabled.  The app can downcast this to
    /// `Arc<std::sync::Mutex<ProjectionSubsystem>>` to add/remove headless
    /// outputs or read back frames at runtime.
    pub projection_handle: Option<Arc<std::sync::Mutex<dyn std::any::Any + Send>>>,
}

impl EngineState {
    /// Create a new state with sensible defaults.
    pub fn new() -> Self {
        Self {
            output_fullscreen: false,
            output_width: 1920,
            output_height: 1080,
            input: InputState::default(),
            input_command: InputCommand::None,
            second_input: InputState::default(),
            second_input_command: InputCommand::None,
            second_input_view: None,
            second_input_sampler: None,
            second_input_generation: 0,
            pick_request: None,
            picked_color: None,
            pixel_pick_armed: false,
            hsb_params: HsbParams::default(),
            hsb_param_bases: HsbParams::default(),
            color_enabled: true,
            audio: AudioState {
                enabled: true,
                amplitude: 1.0,
                smoothing: 0.5,
                normalize: true,
                ..Default::default()
            },
            audio_command: AudioCommand::None,
            audio_routing: AudioRoutingState::new(),
            #[cfg(feature = "ndi")]
            ndi_output: NdiOutputState {
                stream_name: "RustJay".to_string(),
                ..Default::default()
            },
            output_command: OutputCommand::None,
            #[cfg(target_os = "macos")]
            syphon_output: SyphonOutputState {
                server_name: "RustJay".to_string(),
                enabled: false,
            },
            #[cfg(target_os = "windows")]
            spout_output: SpoutOutputState {
                sender_name: "RustJay".to_string(),
                enabled: false,
            },
            #[cfg(target_os = "linux")]
            v4l2_output: V4l2OutputState {
                device_path: "/dev/video12".to_string(),
                enabled: false,
            },
            resolution: ResolutionState::default(),
            performance: Mutex::new(PerformanceMetrics::default()),
            startup_webcam_device: None,
            show_preview: true,
            target_fps: 60,
            ui_scale: 1.0,
            current_tab: GuiTab::Input,
            midi_command: MidiCommand::None,
            modulation_command: ModulationCommand::None,
            midi_available_devices: Vec::new(),
            midi_selected_device: None,
            midi_enabled: false,
            midi_learn_active: false,
            midi_learning_param_name: None,
            midi_mappings: Vec::new(),
            osc_command: OscCommand::None,
            osc_enabled: false,
            osc_host: "127.0.0.1".to_string(),
            osc_port: 9001,
            osc_message_log: Vec::new(),
            preset_command: PresetCommand::None,
            preset_names: Vec::new(),
            preset_quick_slot_names: Default::default(),
            save_settings_requested: false,
            input_discovering: false,
            web_command: WebCommand::None,
            web_enabled: false,
            web_host: "0.0.0.0".to_string(),
            web_port: 8081,
            web_app_name: "rustjay-template".to_string(),
            web_token: String::new(),
            web_full_url: String::new(),
            web_lan_trust: false,
            modulation: {
                let mut eng = ModulationEngine::new();
                // Seed 8 default LFO sources so old presets expecting 8 slots find them.
                for i in 0..8 {
                    let source = crate::modulation::ModulationSource::LFO {
                        waveform: crate::modulation::LFOWaveform::Sine,
                        frequency: 1.0,
                        phase: 0.0,
                        amplitude: 0.5,
                        bipolar: true,
                        tempo_sync: true,
                        division: 2,
                        phase_offset_degrees: 0.0,
                        enabled: true,
                        last_beat_phase: 0.0,
                    };
                    let uuid = format!("lfo_{}", i);
                    eng.add_source_with_uuid(uuid, source);
                    // Default assignments for the first three LFOs (HSB)
                    match i {
                        0 => eng.assign("hue_shift", "lfo_0", 1.0, None),
                        1 => eng.assign("saturation", "lfo_1", 1.0, None),
                        2 => eng.assign("brightness", "lfo_2", 1.0, None),
                        _ => {}
                    }
                }
                Arc::new(Mutex::new(eng))
            },
            modulation_offsets: Vec::new(),
            registered_param_ids: Vec::new(),
            link: LinkState::default(),
            link_command: LinkCommand::None,
            prodj: ProDjState::default(),
            prodj_command: ProDjCommand::None,
            mtc: MtcState::default(),
            sync_source: SyncSource::Audio,
            param_descriptors: Arc::new(Vec::new()),
            custom_param_bases: Vec::new(),
            custom_params: Vec::new(),
            param_osc_addresses: Vec::new(),
            hidden_tabs: Vec::new(),
            param_lookup_prefix: RefCell::new(None),
            param_resolver: None,
            app_state: Arc::new(std::sync::Mutex::new(None)),
            notifications: Arc::new(Mutex::new(Vec::new())),
            next_notification_id: AtomicU64::new(0),
            recording_active: false,
            projection_handle: None,
        }
    }
}

impl EngineState {
    /// Toggle fullscreen mode on the output window.
    pub fn toggle_fullscreen(&mut self) {
        self.output_fullscreen = !self.output_fullscreen;
    }

    /// Post a transient toast notification.
    ///
    /// Toasts auto-dismiss after `duration`; the GUI render loop is responsible
    /// for culling expired entries.
    pub fn notify(
        &self,
        message: impl Into<String>,
        level: NotificationLevel,
        duration: std::time::Duration,
    ) {
        let id = self.next_notification_id.fetch_add(1, Ordering::Relaxed);
        let n = Notification {
            id,
            message: message.into(),
            level,
            expires_at: std::time::Instant::now() + duration,
        };
        match self.notifications.lock() {
            Ok(mut guard) => guard.push(n),
            Err(e) => log::warn!("[notify] notification mutex poisoned: {}", e),
        }
    }

    /// BPM from the user-selected sync source.
    ///
    /// Falls back to audio BPM if the selected source has not yet produced a
    /// valid tempo (e.g. Link enabled but no session state captured yet).
    pub fn effective_bpm(&self) -> f32 {
        match self.sync_source {
            SyncSource::AbletonLink if self.link.bpm > 0.0 => self.link.bpm,
            SyncSource::ProDj if self.prodj.master_bpm > 0.0 => self.prodj.master_bpm,
            _ => self.audio.bpm,
        }
    }

    /// Beat phase (0–1) from the user-selected sync source.
    pub fn effective_beat_phase(&self) -> f32 {
        match self.sync_source {
            SyncSource::AbletonLink if self.link.bpm > 0.0 => self.link.beat_phase,
            SyncSource::ProDj if self.prodj.master_bpm > 0.0 => self.prodj.master_beat_phase,
            _ => self.audio.beat_phase,
        }
    }

    /// Beat phase safe for LFO beat-snap.
    ///
    /// Returns `0.0` when the active sync source is `Audio`. The audio beat
    /// detector resets `beat_phase` to 0 on every detected beat, which fires
    /// the LFO's snap-to-grid at irregular intervals and produces visibly
    /// irregular output. Link and ProDJ supply a stable clock-derived ramp
    /// that wraps predictably once per beat, so the snap is safe there.
    pub fn stable_beat_phase(&self) -> f32 {
        match self.sync_source {
            SyncSource::AbletonLink if self.link.bpm > 0.0 => self.link.beat_phase,
            SyncSource::ProDj if self.prodj.master_bpm > 0.0 => self.prodj.master_beat_phase,
            _ => 0.0,
        }
    }

    /// Human-readable name of the source currently driving tempo.
    pub fn effective_sync_source(&self) -> &'static str {
        self.sync_source.name()
    }

    /// Find the index of a parameter by its string ID.
    pub fn param_index(&self, id: &str) -> Option<usize> {
        self.param_descriptors.iter().position(|d| d.id == id)
    }

    /// Get the modulated value of a parameter (custom or HSB).
    ///
    /// For HSB params (`hue_shift`, `saturation`, `brightness`) returns
    /// `hsb_param_bases + modulation_offset`.
    ///
    /// Linear-scan helper for `modulation_offsets` (small Vec, faster than HashMap for < 10 entries).
    fn modulation_offset(&self, id: &str) -> f32 {
        self.modulation_offsets
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, v)| *v)
            .unwrap_or(0.0)
    }

    /// For custom params: if a modulation assignment exists in
    /// [`modulation_offsets`](Self::modulation_offsets), returns `base + offset * range`.
    /// Otherwise falls back to `custom_params[i]` so audio-routing values (which are
    /// still pre-computed in `update_audio()`) remain visible during the transition.
    ///
    /// If [`param_lookup_prefix`](Self::param_lookup_prefix) is set, the prefix
    /// is prepended to `id` first. This lets nested effects (e.g. a channel's
    /// [`EffectNode`](rustjay_render::EffectNode)) look up their parameters
    /// using bare IDs while the engine stores them under fully-qualified names.
    pub fn get_param(&self, id: &str) -> Option<f32> {
        let resolved = if let Some(prefix) = self.param_lookup_prefix.borrow().as_ref() {
            let prefixed = format!("{prefix}{id}");
            if self.param_index(&prefixed).is_some() {
                prefixed
            } else {
                id.to_string()
            }
        } else {
            id.to_string()
        };

        // HSB params — engine-owned, no descriptor lookup needed
        match resolved.as_str() {
            "hue_shift" => {
                let base = self.hsb_param_bases.hue_shift;
                let offset = self.modulation_offset("hue_shift") * 180.0;
                return Some((base + offset).clamp(-180.0, 180.0));
            }
            "saturation" => {
                let base = self.hsb_param_bases.saturation;
                let offset = self.modulation_offset("saturation");
                return Some((base + offset).clamp(0.0, 2.0));
            }
            "brightness" => {
                let base = self.hsb_param_bases.brightness;
                let offset = self.modulation_offset("brightness");
                return Some((base + offset).clamp(0.0, 2.0));
            }
            _ => {}
        }

        // Custom effect params
        let i = self.param_index(&resolved)?;
        let desc = self.param_descriptors.get(i)?;
        let routed = self.custom_params.get(i).copied()?;
        let range = desc.max - desc.min;
        let offset = self.modulation_offset(&resolved);
        if offset != 0.0 {
            // Sum audio-routing value (in `custom_params`) + modulation offset.
            // When audio routing is inactive, `routed == base`, so this reduces
            // to the base + modulation path.
            Some((routed + offset * range).clamp(desc.min, desc.max))
        } else {
            // No modulation — return the audio-routed (or plain base) value.
            Some(routed)
        }
    }

    /// Get the base (unmodulated) value of a parameter.
    ///
    /// For known engine-owned params (`hue_shift`, `saturation`, `brightness`) returns
    /// the `hsb_param_bases` value. For custom params returns `custom_param_bases[i]`.
    /// For unrecognised keys returns `None`.
    pub fn get_param_base(&self, id: &str) -> Option<f32> {
        match id {
            "hue_shift" => Some(self.hsb_param_bases.hue_shift),
            "saturation" => Some(self.hsb_param_bases.saturation),
            "brightness" => Some(self.hsb_param_bases.brightness),
            _ => self.param_index(id)
                .and_then(|i| self.custom_param_bases.get(i).copied()),
        }
    }

    /// Set the base value of a custom parameter.
    /// Also updates the modulated value so the change is immediately visible
    /// (LFO / audio routing will overwrite on the next frame if active).
    pub fn set_param_base(&mut self, id: &str, value: f32) {
        if let Some(i) = self.param_index(id) {
            self.custom_param_bases[i] = value;
            self.custom_params[i] = value;
        }
    }

    /// Reset modulated params to base values (call before applying LFO + routing each frame).
    pub fn reset_custom_params_to_base(&mut self) {
        if self.custom_params.len() == self.custom_param_bases.len() {
            self.custom_params.copy_from_slice(&self.custom_param_bases);
        } else {
            log::warn!(
                "custom_params length mismatch ({} vs {}), falling back to partial copy",
                self.custom_params.len(),
                self.custom_param_bases.len()
            );
            let min_len = self.custom_params.len().min(self.custom_param_bases.len());
            self.custom_params[..min_len].copy_from_slice(&self.custom_param_bases[..min_len]);
        }
    }

    /// Get parameter descriptors for a given category.
    pub fn params_for_category(&self, category: ParamCategory) -> Vec<&ParameterDescriptor> {
        self.param_descriptors
            .iter()
            .filter(|d| d.category == category)
            .collect()
    }

    /// Get all modulatable (Float / Int) parameter descriptors.
    pub fn modulatable_params(&self) -> Vec<&ParameterDescriptor> {
        self.param_descriptors
            .iter()
            .filter(|d| d.is_modulatable())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_descriptor(id: &str) -> ParameterDescriptor {
        ParameterDescriptor::float(id, id, ParamCategory::Motion, 0.0, 1.0, 0.5, 1.0)
    }

    #[test]
    fn get_param_returns_modulated_value() {
        let mut state = EngineState::new();
        state.param_descriptors = Arc::new(vec![dummy_descriptor("spin")]);
        state.custom_param_bases = vec![0.5];
        state.custom_params = vec![0.5];
        state.modulation_offsets.push(("spin".to_string(), 0.1));

        assert_eq!(state.get_param("spin"), Some(0.6));
    }

    #[test]
    fn get_param_composes_audio_routing_and_modulation() {
        let mut state = EngineState::new();
        state.param_descriptors = Arc::new(vec![dummy_descriptor("spin")]);
        state.custom_param_bases = vec![0.5];
        // Audio routing pushed the value to 0.8
        state.custom_params = vec![0.8];
        state.modulation_offsets.push(("spin".to_string(), 0.1));

        // 0.8 (routed) + 0.1 * 1.0 (range) = 0.9
        assert!((state.get_param("spin").unwrap() - 0.9).abs() < 1e-5);
    }

    #[test]
    fn get_param_returns_routed_value_when_no_modulation() {
        let mut state = EngineState::new();
        state.param_descriptors = Arc::new(vec![dummy_descriptor("spin")]);
        state.custom_param_bases = vec![0.5];
        state.custom_params = vec![0.8];

        assert_eq!(state.get_param("spin"), Some(0.8));
    }

    #[test]
    fn get_param_clamps_to_descriptor_max() {
        let mut state = EngineState::new();
        state.param_descriptors = Arc::new(vec![dummy_descriptor("spin")]);
        state.custom_param_bases = vec![0.5];
        state.custom_params = vec![0.5];
        state.modulation_offsets.push(("spin".to_string(), 0.6));

        // 0.5 + 0.6 * 1.0 = 1.1 → clamped to 1.0
        assert_eq!(state.get_param("spin"), Some(1.0));
    }

    #[test]
    fn get_param_base_returns_unmodulated_value() {
        let mut state = EngineState::new();
        state.param_descriptors = Arc::new(vec![dummy_descriptor("spin")]);
        state.custom_param_bases = vec![0.5];
        state.custom_params = vec![0.8];
        state.modulation_offsets.push(("spin".to_string(), 0.1));

        assert_eq!(state.get_param_base("spin"), Some(0.5));
    }

    #[test]
    fn registered_param_ids_empty_by_default() {
        let state = EngineState::new();
        assert!(state.registered_param_ids.is_empty());
    }
}

impl Default for EngineState {
    fn default() -> Self {
        Self::new()
    }
}
