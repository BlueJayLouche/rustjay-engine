use serde::{Deserialize, Serialize};
use crate::lfo::LfoState;
use crate::routing::AudioRoutingState;

// ── Command enums ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum InputCommand {
    None,
    StartWebcam { device_index: usize, width: u32, height: u32, fps: u32 },
    #[cfg(feature = "ndi")]
    StartNdi { source_name: String },
    #[cfg(target_os = "macos")]
    StartSyphon { server_name: String, server_uuid: String },
    #[cfg(target_os = "windows")]
    StartSpout { sender_name: String },
    #[cfg(target_os = "linux")]
    StartV4l2 { device_path: String },
    StopInput,
    RefreshDevices,
}

impl Default for InputCommand { fn default() -> Self { Self::None } }

#[derive(Debug, Clone, PartialEq)]
pub enum OutputCommand {
    None,
    #[cfg(feature = "ndi")]
    StartNdi,
    #[cfg(feature = "ndi")]
    StopNdi,
    #[cfg(target_os = "macos")]
    StartSyphon,
    #[cfg(target_os = "macos")]
    StopSyphon,
    #[cfg(target_os = "windows")]
    StartSpout { sender_name: String },
    #[cfg(target_os = "windows")]
    StopSpout,
    #[cfg(target_os = "linux")]
    StartV4l2 { device_path: String },
    #[cfg(target_os = "linux")]
    StopV4l2,
    ResizeOutput,
}

impl Default for OutputCommand { fn default() -> Self { Self::None } }

#[derive(Debug, Clone, PartialEq)]
pub enum AudioCommand {
    None, Start, Stop, RefreshDevices, SelectDevice(String), SetFftSize(usize),
}
impl Default for AudioCommand { fn default() -> Self { Self::None } }

#[derive(Debug, Clone, PartialEq)]
pub enum MidiCommand {
    None,
    RefreshDevices,
    SelectDevice(String),
    StartLearn { param_path: String, param_name: String },
    CancelLearn,
    ClearMappings,
}
impl Default for MidiCommand { fn default() -> Self { Self::None } }

#[derive(Debug, Clone, PartialEq)]
pub enum OscCommand { None, Start, Stop, SetPort(u16), RefreshAddresses }
impl Default for OscCommand { fn default() -> Self { Self::None } }

#[derive(Debug, Clone, PartialEq)]
pub enum PresetCommand {
    None,
    Save { name: String },
    Load(usize),
    Delete(usize),
    ApplySlot(usize),
    AssignSlot { preset_index: usize, slot: usize },
    Refresh,
}
impl Default for PresetCommand { fn default() -> Self { Self::None } }

#[derive(Debug, Clone, PartialEq)]
pub enum WebCommand { None, Start, Stop, SetPort(u16) }
impl Default for WebCommand { fn default() -> Self { Self::None } }

// ── Input type ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputType {
    None,
    Webcam,
    #[cfg(feature = "ndi")]
    Ndi,
    #[cfg(target_os = "macos")]
    Syphon,
    #[cfg(target_os = "windows")]
    Spout,
    #[cfg(target_os = "linux")]
    V4l2,
}

impl Default for InputType { fn default() -> Self { Self::None } }

impl InputType {
    pub fn name(&self) -> &'static str {
        match self {
            InputType::None   => "None",
            InputType::Webcam => "Webcam",
            #[cfg(feature = "ndi")]
            InputType::Ndi    => "NDI",
            #[cfg(target_os = "macos")]
            InputType::Syphon => "Syphon",
            #[cfg(target_os = "windows")]
            InputType::Spout  => "Spout",
            #[cfg(target_os = "linux")]
            InputType::V4l2   => "V4L2",
        }
    }
}

// ── Sub-states ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct InputState {
    pub input_type: InputType,
    pub source_name: String,
    pub is_active: bool,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HsbParams {
    pub hue_shift: f32,
    pub saturation: f32,
    pub brightness: f32,
}

impl Default for HsbParams {
    fn default() -> Self { Self { hue_shift: 0.0, saturation: 1.0, brightness: 1.0 } }
}

impl HsbParams {
    pub fn reset(&mut self) { *self = Self::default(); }
}

#[derive(Debug, Clone)]
pub struct AudioState {
    pub fft: [f32; 8],
    pub volume: f32,
    pub beat: bool,
    pub bpm: f32,
    pub beat_phase: f32,
    pub enabled: bool,
    pub amplitude: f32,
    pub smoothing: f32,
    pub selected_device: Option<String>,
    pub available_devices: Vec<String>,
    pub normalize: bool,
    pub pink_noise_shaping: bool,
    pub fft_size: usize,
    pub tap_times: Vec<f64>,
    pub last_tap_time: f64,
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

#[cfg(feature = "ndi")]
#[derive(Debug, Clone, Default)]
pub struct NdiOutputState {
    pub stream_name: String,
    pub is_active: bool,
    pub include_alpha: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SyphonOutputState { pub server_name: String, pub enabled: bool }

#[derive(Debug, Clone, Default)]
pub struct SpoutOutputState { pub sender_name: String, pub enabled: bool }

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Default)]
pub struct V4l2OutputState { pub device_path: String, pub enabled: bool }

#[derive(Debug, Clone)]
pub struct ResolutionState {
    pub internal_width: u32,
    pub internal_height: u32,
    pub input_width: u32,
    pub input_height: u32,
}

impl Default for ResolutionState {
    fn default() -> Self {
        Self { internal_width: 1920, internal_height: 1080, input_width: 1920, input_height: 1080 }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PerformanceMetrics { pub fps: f32, pub frame_time_ms: f32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GuiTab {
    #[default] Input, Color, Audio, Output, Presets, Midi, Osc, Web, Settings,
}

impl GuiTab {
    pub fn name(&self) -> &'static str {
        match self {
            GuiTab::Input    => "Input",
            GuiTab::Color    => "Color",
            GuiTab::Audio    => "Audio",
            GuiTab::Output   => "Output",
            GuiTab::Presets  => "Presets",
            GuiTab::Midi     => "MIDI",
            GuiTab::Osc      => "OSC",
            GuiTab::Web      => "Web",
            GuiTab::Settings => "Settings",
        }
    }
}

// ── EngineState ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct EngineState {
    pub output_fullscreen: bool,
    pub output_width: u32,
    pub output_height: u32,

    pub input: InputState,
    pub input_command: InputCommand,

    pub hsb_params: HsbParams,
    pub color_enabled: bool,

    pub audio: AudioState,
    pub audio_command: AudioCommand,
    pub audio_routing: AudioRoutingState,

    pub lfo: LfoState,

    #[cfg(feature = "ndi")]
    pub ndi_output: NdiOutputState,
    pub output_command: OutputCommand,

    #[cfg(target_os = "macos")]
    pub syphon_output: SyphonOutputState,

    #[cfg(target_os = "windows")]
    pub spout_output: SpoutOutputState,

    #[cfg(target_os = "linux")]
    pub v4l2_output: V4l2OutputState,

    pub resolution: ResolutionState,
    pub performance: PerformanceMetrics,

    pub show_preview: bool,
    pub ui_scale: f32,
    pub current_tab: GuiTab,

    pub midi_command: MidiCommand,
    pub osc_command: OscCommand,
    pub osc_enabled: bool,
    pub osc_port: u16,

    pub preset_command: PresetCommand,
    pub preset_names: Vec<String>,
    pub preset_quick_slot_names: [Option<String>; 8],

    pub save_settings_requested: bool,
    pub input_discovering: bool,

    pub web_command: WebCommand,
    pub web_enabled: bool,
    pub web_port: u16,
}

impl EngineState {
    pub fn new() -> Self {
        Self {
            output_fullscreen: false,
            output_width: 1920,
            output_height: 1080,
            input: InputState::default(),
            input_command: InputCommand::None,
            hsb_params: HsbParams::default(),
            color_enabled: true,
            audio: AudioState { enabled: true, amplitude: 1.0, smoothing: 0.5, normalize: true, ..Default::default() },
            audio_command: AudioCommand::None,
            audio_routing: AudioRoutingState::new(),
            #[cfg(feature = "ndi")]
            ndi_output: NdiOutputState { stream_name: "RustJay".to_string(), ..Default::default() },
            output_command: OutputCommand::None,
            #[cfg(target_os = "macos")]
            syphon_output: SyphonOutputState { server_name: "RustJay".to_string(), enabled: false },
            #[cfg(target_os = "windows")]
            spout_output: SpoutOutputState { sender_name: "RustJay".to_string(), enabled: false },
            #[cfg(target_os = "linux")]
            v4l2_output: V4l2OutputState { device_path: "/dev/video12".to_string(), enabled: false },
            resolution: ResolutionState::default(),
            performance: PerformanceMetrics::default(),
            show_preview: true,
            ui_scale: 1.0,
            current_tab: GuiTab::Input,
            midi_command: MidiCommand::None,
            osc_command: OscCommand::None,
            osc_enabled: false,
            osc_port: 9001,
            preset_command: PresetCommand::None,
            preset_names: Vec::new(),
            preset_quick_slot_names: Default::default(),
            save_settings_requested: false,
            input_discovering: false,
            web_command: WebCommand::None,
            web_enabled: false,
            web_port: 8081,
            lfo: LfoState::new(),
        }
    }

    pub fn toggle_fullscreen(&mut self) {
        self.output_fullscreen = !self.output_fullscreen;
    }
}

impl Default for EngineState {
    fn default() -> Self { Self::new() }
}
