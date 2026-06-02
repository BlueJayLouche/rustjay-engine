//! # Presets System
//!
//! Save and load parameter snapshots with quick preset selector.

use rustjay_core::{HsbParams, LfoBank, EngineState, MidiCommand, MidiMappingSnapshot, RoutingMatrix};
use serde::{Deserialize, Serialize};

fn default_fft_size() -> usize {
    rustjay_audio::DEFAULT_FFT_SIZE
}

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A single preset containing all parameter values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    /// Preset name
    pub name: String,
    /// Creation/modification timestamp
    pub timestamp: u64,
    /// Description/notes
    pub description: String,
    
    // Color parameters
    /// Base HSB color parameters.
    #[serde(default)]
    pub hsb_params: HsbParams,
    /// Whether color processing is enabled.
    #[serde(default)]
    pub color_enabled: bool,

    // Audio parameters
    /// Global audio amplitude multiplier.
    #[serde(default)]
    pub audio_amplitude: f32,
    /// Audio smoothing factor.
    #[serde(default)]
    pub audio_smoothing: f32,
    /// Whether audio normalization is enabled.
    #[serde(default)]
    pub audio_normalize: bool,
    /// Whether pink noise shaping is applied.
    #[serde(default)]
    pub audio_pink_noise: bool,
    /// FFT size for audio analysis.
    #[serde(default = "default_fft_size")]
    pub audio_fft_size: usize,

    // Resolution
    /// Internal render width.
    pub internal_width: u32,
    /// Internal render height.
    pub internal_height: u32,

    // LFO settings
    /// Bank of LFOs for modulation.
    #[serde(default)]
    pub lfo_bank: LfoBank,

    // Audio routing settings
    /// Matrix mapping audio features to parameters.
    #[serde(default)]
    pub routing_matrix: RoutingMatrix,
    /// Whether audio-driven parameter routing is active.
    #[serde(default)]
    pub audio_routing_enabled: bool,

    // Custom parameters (for extensibility)
    /// Custom user-defined parameter values.
    #[serde(default)]
    pub custom_values: HashMap<String, f32>,

    /// MIDI CC/Note/Aftertouch mappings.
    #[serde(default)]
    pub midi_mappings: Vec<MidiMappingSnapshot>,

    /// Optional plugin-specific state serialized as JSON.
    #[serde(default)]
    pub plugin_state: Option<String>,
}

impl Preset {
    /// Create a new preset from current state
    pub fn from_state(name: &str, state: &EngineState) -> Self {
        Self {
            name: name.to_string(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            description: String::new(),
            hsb_params: HsbParams {
                hue_shift: state.audio_routing.base_hue,
                saturation: state.audio_routing.base_saturation,
                brightness: state.audio_routing.base_brightness,
            },
            color_enabled: state.color_enabled,
            audio_amplitude: state.audio.amplitude,
            audio_smoothing: state.audio.smoothing,
            audio_normalize: state.audio.normalize,
            audio_pink_noise: state.audio.pink_noise_shaping,
            audio_fft_size: state.audio.fft_size,
            internal_width: state.resolution.internal_width,
            internal_height: state.resolution.internal_height,
            lfo_bank: state.lfo.bank.clone(),
            routing_matrix: state.audio_routing.matrix.clone(),
            audio_routing_enabled: state.audio_routing.enabled,
            custom_values: state.param_descriptors.iter().enumerate()
                .map(|(i, d)| (d.id.clone(), state.custom_param_bases[i]))
                .collect(),
            midi_mappings: state.midi_mappings.clone(),
            plugin_state: None,
        }
    }
    
    /// Apply this preset to the shared state
    pub fn apply_to_state(&self, state: &mut EngineState) {
        state.hsb_params = self.hsb_params;
        state.audio_routing.update_base_values(
            self.hsb_params.hue_shift,
            self.hsb_params.saturation,
            self.hsb_params.brightness,
        );
        state.color_enabled = self.color_enabled;
        state.audio.amplitude = self.audio_amplitude;
        state.audio.smoothing = self.audio_smoothing;
        state.audio.normalize = self.audio_normalize;
        state.audio.pink_noise_shaping = self.audio_pink_noise;
        state.audio.fft_size = self.audio_fft_size;
        state.resolution.internal_width = self.internal_width;
        state.resolution.internal_height = self.internal_height;
        state.lfo.bank = self.lfo_bank.clone();
        state.audio_routing.matrix = self.routing_matrix.clone();
        state.audio_routing.enabled = self.audio_routing_enabled;
        // Restore custom parameter values
        for (id, value) in &self.custom_values {
            if let Some(i) = state.param_descriptors.iter().position(|d| &d.id == id) {
                state.custom_param_bases[i] = *value;
                state.custom_params[i] = *value;
            } else {
                log::warn!("Preset parameter '{}' not found in current descriptors, skipping", id);
            }
        }
        // Restore MIDI mappings via command (engine rebuilds MidiState on next frame).
        if !self.midi_mappings.is_empty() {
            state.midi_command = MidiCommand::RestoreMappings(self.midi_mappings.clone());
        }
    }
    
    /// Save preset to file
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }
    
    /// Load preset from file
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        // Limit file size to mitigate stack-overflow DoS from deeply nested JSON.
        if metadata.len() > 65_536 {
            return Err(anyhow::anyhow!("Preset file too large: {} bytes (max 64 KiB)", metadata.len()));
        }
        let content = std::fs::read_to_string(path)?;
        let preset: Preset = serde_json::from_str(&content)?;
        preset.validate()?;
        Ok(preset)
    }

    /// Validate preset fields are within acceptable ranges.
    fn validate(&self) -> anyhow::Result<()> {
        const MAX_DIM: u32 = 4096;
        const VALID_FFT_SIZES: &[usize] = &[1024, 2048, 4096, 8192];

        if self.internal_width > MAX_DIM || self.internal_height > MAX_DIM {
            return Err(anyhow::anyhow!(
                "Preset dimensions out of range: {}x{} (max {})",
                self.internal_width, self.internal_height, MAX_DIM
            ));
        }
        if !VALID_FFT_SIZES.contains(&self.audio_fft_size) {
            return Err(anyhow::anyhow!(
                "Invalid audio_fft_size: {} (valid: {:?})",
                self.audio_fft_size, VALID_FFT_SIZES
            ));
        }
        if self.custom_values.len() > 256 {
            return Err(anyhow::anyhow!(
                "Too many custom values: {} (max 256)",
                self.custom_values.len()
            ));
        }
        Ok(())
    }
    
    /// Get filename-safe version of name
    pub fn safe_filename(&self) -> String {
        self.name
            .to_lowercase()
            .replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "_")
            .replace("__", "_")
            .trim_matches('_')
            .to_string()
    }
}

/// Bank of presets with quick access slots
#[derive(Debug, Clone)]
pub struct PresetBank {
    /// All available presets
    pub presets: Vec<Preset>,
    /// Quick access slots (indices into presets)
    pub quick_slots: [Option<usize>; 8],
    /// Currently selected preset index
    pub current_index: Option<usize>,
    /// Presets directory
    pub presets_dir: PathBuf,
}

impl PresetBank {
    /// Create a new preset bank backed by the given directory.
    pub fn new(presets_dir: PathBuf) -> Self {
        let mut bank = Self {
            presets: Vec::new(),
            quick_slots: [None; 8],
            current_index: None,
            presets_dir,
        };
        
        // Try to load existing presets
        if let Err(e) = bank.refresh() {
            log::warn!("[PresetBank] Failed to refresh presets: {}", e);
        }
        
        bank
    }
    
    /// Refresh preset list from disk
    pub fn refresh(&mut self) -> anyhow::Result<()> {
        self.presets.clear();
        
        if !self.presets_dir.exists() {
            std::fs::create_dir_all(&self.presets_dir)?;
            return Ok(());
        }
        
        let entries = std::fs::read_dir(&self.presets_dir)?;
        
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                match Preset::load(&path) {
                    Ok(preset) => {
                        self.presets.push(preset);
                    }
                    Err(e) => {
                        log::warn!("Failed to load preset {:?}: {}", path, e);
                    }
                }
            }
        }
        
        // Sort by name
        self.presets.sort_by(|a, b| a.name.cmp(&b.name));
        
        log::info!("Loaded {} presets", self.presets.len());
        Ok(())
    }
    
    /// Add a new preset
    pub fn add_preset(&mut self, preset: Preset) -> anyhow::Result<usize> {
        let filename = format!("{}.json", preset.safe_filename());
        let path = self.presets_dir.join(&filename);
        
        preset.save(&path)?;
        
        self.presets.push(preset);
        self.presets.sort_by(|a, b| a.name.cmp(&b.name));
        
        // Find the index of the new preset
        let index = self.presets.iter().position(|p| {
            let p_filename = format!("{}.json", p.safe_filename());
            p_filename == filename
        }).unwrap_or(self.presets.len() - 1);
        
        log::info!("Saved preset '{}' at index {}", self.presets[index].name, index);
        Ok(index)
    }
    
    /// Delete a preset
    pub fn delete_preset(&mut self, index: usize) -> anyhow::Result<()> {
        if index >= self.presets.len() {
            return Err(anyhow::anyhow!("Invalid preset index"));
        }
        
        let preset = &self.presets[index];
        let filename = format!("{}.json", preset.safe_filename());
        let path = self.presets_dir.join(&filename);
        
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        
        // Remove from quick slots
        for slot in &mut self.quick_slots {
            if *slot == Some(index) {
                *slot = None;
            } else if let Some(idx) = *slot {
                if idx > index {
                    *slot = Some(idx - 1);
                }
            }
        }
        
        // Adjust current index
        if let Some(current) = self.current_index {
            if current == index {
                self.current_index = None;
            } else if current > index {
                self.current_index = Some(current - 1);
            }
        }
        
        self.presets.remove(index);
        
        log::info!("Deleted preset at index {}", index);
        Ok(())
    }
    
    /// Get a preset by index
    pub fn get(&self, index: usize) -> Option<&Preset> {
        self.presets.get(index)
    }
    
    /// Get mutable preset by index
    pub fn get_mut(&mut self, index: usize) -> Option<&mut Preset> {
        self.presets.get_mut(index)
    }
    
    /// Assign a preset to a quick slot (1-8)
    pub fn assign_to_slot(&mut self, preset_index: usize, slot: usize) -> anyhow::Result<()> {
        if slot < 1 || slot > 8 {
            return Err(anyhow::anyhow!("Slot must be 1-8"));
        }
        if preset_index >= self.presets.len() {
            return Err(anyhow::anyhow!("Invalid preset index"));
        }
        
        self.quick_slots[slot - 1] = Some(preset_index);
        log::info!("Assigned preset '{}' to quick slot {}", 
            self.presets[preset_index].name, slot);
        Ok(())
    }
    
    /// Clear a quick slot
    pub fn clear_slot(&mut self, slot: usize) {
        if slot >= 1 && slot <= 8 {
            self.quick_slots[slot - 1] = None;
        }
    }
    
    /// Get preset index for a quick slot
    pub fn get_slot(&self, slot: usize) -> Option<usize> {
        if slot >= 1 && slot <= 8 {
            self.quick_slots[slot - 1]
        } else {
            None
        }
    }
    
    /// Get preset name for a quick slot
    pub fn get_slot_name(&self, slot: usize) -> Option<&str> {
        self.get_slot(slot).and_then(|idx| {
            self.presets.get(idx).map(|p| p.name.as_str())
        })
    }
    
    /// Apply preset by index
    pub fn apply_preset(&mut self, index: usize, state: &mut EngineState) -> anyhow::Result<()> {
        if let Some(preset) = self.presets.get(index) {
            preset.apply_to_state(state);
            self.current_index = Some(index);
            log::info!("Applied preset: {}", preset.name);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Invalid preset index: {}", index))
        }
    }

    /// Apply preset from quick slot
    pub fn apply_slot(&mut self, slot: usize, state: &mut EngineState) -> anyhow::Result<()> {
        if let Some(index) = self.get_slot(slot) {
            self.apply_preset(index, state)
        } else {
            Err(anyhow::anyhow!("Quick slot {} is empty", slot))
        }
    }
    
    /// Get current preset name
    pub fn current_name(&self) -> Option<&str> {
        self.current_index.and_then(|idx| {
            self.presets.get(idx).map(|p| p.name.as_str())
        })
    }
    
    /// Export preset to a specific path
    pub fn export_preset(&self, index: usize, path: &Path) -> anyhow::Result<()> {
        if let Some(preset) = self.presets.get(index) {
            preset.save(path)?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Invalid preset index"))
        }
    }
    
    /// Import preset from path
    pub fn import_preset(&mut self, path: &Path) -> anyhow::Result<usize> {
        let preset = Preset::load(path)?;
        self.add_preset(preset)
    }
    
    /// Update existing preset with current state
    pub fn update_preset(&mut self, index: usize, state: &EngineState) -> anyhow::Result<()> {
        if index >= self.presets.len() {
            return Err(anyhow::anyhow!("Invalid preset index"));
        }

        let name = self.presets[index].name.clone();
        let plugin_state = self.presets[index].plugin_state.clone();
        let mut preset = Preset::from_state(&name, state);
        preset.plugin_state = plugin_state;
        preset.description = self.presets[index].description.clone();
        
        // Save to disk
        let filename = format!("{}.json", preset.safe_filename());
        let path = self.presets_dir.join(&filename);
        preset.save(&path)?;
        
        // Update in memory
        self.presets[index] = preset;
        
        log::info!("Updated preset: {}", name);
        Ok(())
    }
    
    /// Duplicate a preset
    pub fn duplicate_preset(&mut self, index: usize, new_name: &str) -> anyhow::Result<usize> {
        if index >= self.presets.len() {
            return Err(anyhow::anyhow!("Invalid preset index"));
        }
        
        let mut preset = self.presets[index].clone();
        preset.name = new_name.to_string();
        preset.timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        self.add_preset(preset)
    }
    
    /// Rename a preset
    pub fn rename_preset(&mut self, index: usize, new_name: &str) -> anyhow::Result<()> {
        if index >= self.presets.len() {
            return Err(anyhow::anyhow!("Invalid preset index"));
        }
        
        // Delete old file
        let old_filename = format!("{}.json", self.presets[index].safe_filename());
        let old_path = self.presets_dir.join(&old_filename);
        if old_path.exists() {
            std::fs::remove_file(&old_path)?;
        }
        
        // Update and save
        self.presets[index].name = new_name.to_string();
        let new_filename = format!("{}.json", self.presets[index].safe_filename());
        let new_path = self.presets_dir.join(&new_filename);
        self.presets[index].save(&new_path)?;
        
        // Re-sort
        self.presets.sort_by(|a, b| a.name.cmp(&b.name));
        
        log::info!("Renamed preset to: {}", new_name);
        Ok(())
    }
}

/// Get the presets directory for a specific app (isolated per-app).
pub fn presets_dir_for(app_name: &str) -> anyhow::Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?;
    Ok(config_dir.join("rustjay").join(app_name).join("presets"))
}
