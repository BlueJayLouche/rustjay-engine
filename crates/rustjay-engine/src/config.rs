//! Persistent configuration — save/load application settings.

use rustjay_core::{HsbParams, EngineState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

fn default_fft_size() -> usize {
    rustjay_audio::DEFAULT_FFT_SIZE
}

fn default_target_fps() -> u32 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct MidiMappingConfig {
    pub cc: u8,
    pub channel: u8,
    pub param_path: String,
    pub min_value: f32,
    pub max_value: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OscConfig {
    pub host: String,
    pub port: u16,
    pub enabled: bool,
    pub base_address: String,
}

impl Default for OscConfig {
    fn default() -> Self {
        Self { host: "127.0.0.1".to_string(), port: 9001, enabled: false, base_address: "/rustjay".to_string() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AppSettings {
    pub output_width: u32,
    pub output_height: u32,
    pub internal_width: u32,
    pub internal_height: u32,
    pub hsb_params: HsbParams,
    pub color_enabled: bool,
    pub audio_enabled: bool,
    pub audio_amplitude: f32,
    pub audio_smoothing: f32,
    pub audio_normalize: bool,
    pub audio_pink_noise: bool,
    #[serde(default = "default_fft_size")]
    pub audio_fft_size: usize,
    pub audio_device: Option<String>,
    #[cfg(feature = "ndi")]
    pub ndi_stream_name: String,
    #[cfg(feature = "ndi")]
    pub ndi_include_alpha: bool,
    #[cfg(target_os = "macos")]
    pub syphon_server_name: String,
    #[cfg(target_os = "windows")]
    pub spout_output_name: String,
    #[cfg(target_os = "linux")]
    pub v4l2_device_path: String,
    pub midi_enabled: bool,
    pub midi_device: Option<String>,
    pub midi_mappings: Vec<MidiMappingConfig>,
    pub osc: OscConfig,
    pub web_host: String,
    pub web_port: u16,
    pub ui_scale: f32,
    pub show_preview: bool,
    #[serde(default = "default_target_fps")]
    pub target_fps: u32,
    /// Effect-declared custom parameter values.
    #[serde(default)]
    pub custom_params: HashMap<String, f32>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            output_width: 1920,
            output_height: 1080,
            internal_width: 1920,
            internal_height: 1080,
            hsb_params: HsbParams::default(),
            color_enabled: true,
            audio_enabled: true,
            audio_amplitude: 1.0,
            audio_smoothing: 0.5,
            audio_normalize: true,
            audio_pink_noise: false,
            audio_fft_size: rustjay_audio::DEFAULT_FFT_SIZE,
            audio_device: None,
            #[cfg(feature = "ndi")]
            ndi_stream_name: "RustJay".to_string(),
            #[cfg(feature = "ndi")]
            ndi_include_alpha: false,
            #[cfg(target_os = "macos")]
            syphon_server_name: "RustJay".to_string(),
            #[cfg(target_os = "windows")]
            spout_output_name: "RustJay".to_string(),
            #[cfg(target_os = "linux")]
            v4l2_device_path: "/dev/video12".to_string(),
            midi_enabled: false,
            midi_device: None,
            midi_mappings: Vec::new(),
            osc: OscConfig::default(),
            web_host: "127.0.0.1".to_string(),
            web_port: 8081,
            ui_scale: 1.0,
            show_preview: true,
            target_fps: 60,
            custom_params: HashMap::new(),
        }
    }
}

impl AppSettings {
    pub fn load(app_name: &str) -> anyhow::Result<Self> {
        let path = Self::config_path(app_name)?;
        let tmp_path = path.with_extension("json.tmp");
        if tmp_path.exists() {
            log::warn!("Found leftover {:?} — previous save may have been interrupted.", tmp_path);
        }
        if !path.exists() {
            log::info!("No config file found at {:?}, using defaults", path);
            return Ok(Self::default());
        }
        let metadata = std::fs::metadata(&path)?;
        // Limit file size to mitigate stack-overflow DoS from deeply nested JSON.
        if metadata.len() > 65_536 {
            return Err(anyhow::anyhow!("Config file too large: {} bytes (max 64 KiB)", metadata.len()));
        }
        let content = std::fs::read_to_string(&path)?;
        let settings: AppSettings = serde_json::from_str(&content)?;
        settings.validate()?;
        log::info!("Loaded settings from {:?}", path);
        Ok(settings)
    }

    /// Validate config fields are within acceptable ranges.
    fn validate(&self) -> anyhow::Result<()> {
        const MAX_DIM: u32 = 4096;
        const VALID_FFT_SIZES: &[usize] = &[1024, 2048, 4096, 8192];

        if self.internal_width > MAX_DIM || self.internal_height > MAX_DIM
            || self.output_width > MAX_DIM || self.output_height > MAX_DIM
        {
            return Err(anyhow::anyhow!(
                "Output/internal dimensions out of range (max {})",
                MAX_DIM
            ));
        }
        if !VALID_FFT_SIZES.contains(&self.audio_fft_size) {
            return Err(anyhow::anyhow!(
                "Invalid audio_fft_size: {} (valid: {:?})",
                self.audio_fft_size, VALID_FFT_SIZES
            ));
        }
        if self.custom_params.len() > 256 {
            return Err(anyhow::anyhow!(
                "Too many custom params: {} (max 256)",
                self.custom_params.len()
            ));
        }
        const VALID_FPS: &[u32] = &[24, 30, 48, 60, 90, 120];
        if !VALID_FPS.contains(&self.target_fps) {
            return Err(anyhow::anyhow!(
                "Invalid target_fps: {} (valid: {:?})",
                self.target_fps, VALID_FPS
            ));
        }
        Ok(())
    }

    pub fn save(&self, app_name: &str) -> anyhow::Result<()> {
        let path = Self::config_path(app_name)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp_path = path.with_extension("json.tmp");
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&tmp_path, &content)?;
        std::fs::rename(&tmp_path, &path)?;
        log::info!("Saved settings to {:?}", path);
        Ok(())
    }

    pub fn config_path(app_name: &str) -> anyhow::Result<PathBuf> {
        let dirs = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?;
        Ok(dirs.join("rustjay").join(format!("{}.json", app_name)))
    }

    pub fn apply_to_state(&self, state: &mut EngineState) {
        state.output_width = self.output_width;
        state.output_height = self.output_height;
        state.resolution.internal_width = self.internal_width;
        state.resolution.internal_height = self.internal_height;
        state.hsb_params = self.hsb_params;
        state.color_enabled = self.color_enabled;
        state.audio.enabled = self.audio_enabled;
        state.audio.amplitude = self.audio_amplitude;
        state.audio.smoothing = self.audio_smoothing;
        state.audio.normalize = self.audio_normalize;
        state.audio.pink_noise_shaping = self.audio_pink_noise;
        state.audio.fft_size = self.audio_fft_size;
        state.audio.selected_device = self.audio_device.clone();
        #[cfg(feature = "ndi")]
        {
            state.ndi_output.stream_name = self.ndi_stream_name.clone();
            state.ndi_output.include_alpha = self.ndi_include_alpha;
        }
        #[cfg(target_os = "macos")]
        {
            state.syphon_output.server_name = self.syphon_server_name.clone();
        }
        #[cfg(target_os = "windows")]
        {
            state.spout_output.sender_name = self.spout_output_name.clone();
        }
        #[cfg(target_os = "linux")]
        {
            state.v4l2_output.device_path = self.v4l2_device_path.clone();
        }
        state.osc_host = self.osc.host.clone();
        state.osc_port = self.osc.port;
        state.web_host = self.web_host.clone();
        state.web_port = self.web_port;
        state.ui_scale = self.ui_scale;
        state.show_preview = self.show_preview;
        state.target_fps = self.target_fps;
        // Restore custom param values (only for params that are declared)
        for (id, value) in &self.custom_params {
            if let Some(i) = state.param_descriptors.iter().position(|d| &d.id == id) {
                state.custom_param_bases[i] = *value;
                state.custom_params[i] = *value;
            } else {
                log::warn!("Config parameter '{}' not found in current descriptors, skipping", id);
            }
        }
    }

    pub fn from_state(state: &EngineState) -> Self {
        Self {
            output_width: state.output_width,
            output_height: state.output_height,
            internal_width: state.resolution.internal_width,
            internal_height: state.resolution.internal_height,
            hsb_params: state.hsb_params,
            color_enabled: state.color_enabled,
            audio_enabled: state.audio.enabled,
            audio_amplitude: state.audio.amplitude,
            audio_smoothing: state.audio.smoothing,
            audio_normalize: state.audio.normalize,
            audio_pink_noise: state.audio.pink_noise_shaping,
            audio_fft_size: state.audio.fft_size,
            audio_device: state.audio.selected_device.clone(),
            #[cfg(feature = "ndi")]
            ndi_stream_name: state.ndi_output.stream_name.clone(),
            #[cfg(feature = "ndi")]
            ndi_include_alpha: state.ndi_output.include_alpha,
            #[cfg(target_os = "macos")]
            syphon_server_name: state.syphon_output.server_name.clone(),
            #[cfg(target_os = "windows")]
            spout_output_name: state.spout_output.sender_name.clone(),
            #[cfg(target_os = "linux")]
            v4l2_device_path: state.v4l2_output.device_path.clone(),
            midi_enabled: false,
            midi_device: None,
            midi_mappings: Vec::new(),
            osc: OscConfig {
                host: state.osc_host.clone(),
                port: state.osc_port,
                enabled: state.osc_enabled,
                base_address: "/rustjay".to_string(),
            },
            web_host: state.web_host.clone(),
            web_port: state.web_port,
            ui_scale: state.ui_scale,
            show_preview: state.show_preview,
            target_fps: state.target_fps,
            custom_params: state.param_descriptors.iter().enumerate()
                .map(|(i, d)| (d.id.clone(), state.custom_param_bases[i]))
                .collect(),
        }
    }
}

pub(crate) struct ConfigManager {
    pub settings: AppSettings,
    pub app_name: String,
}

impl ConfigManager {
    pub fn new(app_name: &str) -> Self {
        let settings = AppSettings::load(app_name).unwrap_or_default();
        Self { settings, app_name: app_name.to_string() }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.settings.save(&self.app_name)
    }
}
