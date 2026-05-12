//! # Audio Routing System
//!
//! Routes audio FFT bands to various parameters for audio-reactive visuals.
//! Adapted from rustjay-delta for HSB color parameters.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// FFT frequency bands (8-band spectrum)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FftBand {
    /// 20–60 Hz.
    SubBass = 0,
    /// 60–120 Hz.
    Bass = 1,
    /// 120–250 Hz.
    LowMid = 2,
    /// 250–500 Hz.
    Mid = 3,
    /// 500–2000 Hz.
    HighMid = 4,
    /// 2000–4000 Hz.
    High = 5,
    /// 4000–8000 Hz.
    VeryHigh = 6,
    /// 8000–16000 Hz.
    Presence = 7,
}

impl FftBand {
    /// Human-readable band name.
    pub fn name(&self) -> &'static str {
        match self {
            FftBand::SubBass => "Sub Bass",
            FftBand::Bass => "Bass",
            FftBand::LowMid => "Low Mid",
            FftBand::Mid => "Mid",
            FftBand::HighMid => "High Mid",
            FftBand::High => "High",
            FftBand::VeryHigh => "Very High",
            FftBand::Presence => "Presence",
        }
    }
    
    /// Abbreviated band name for compact UIs.
    pub fn short_name(&self) -> &'static str {
        match self {
            FftBand::SubBass => "Sub",
            FftBand::Bass => "Bass",
            FftBand::LowMid => "LoMid",
            FftBand::Mid => "Mid",
            FftBand::HighMid => "HiMid",
            FftBand::High => "High",
            FftBand::VeryHigh => "VHigh",
            FftBand::Presence => "Presence",
        }
    }
    
    /// All frequency bands in order.
    pub fn all() -> &'static [FftBand] {
        &[
            FftBand::SubBass,
            FftBand::Bass,
            FftBand::LowMid,
            FftBand::Mid,
            FftBand::HighMid,
            FftBand::High,
            FftBand::VeryHigh,
            FftBand::Presence,
        ]
    }
    
    /// Convert a band index (0–7) to an `FftBand`.
    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(FftBand::SubBass),
            1 => Some(FftBand::Bass),
            2 => Some(FftBand::LowMid),
            3 => Some(FftBand::Mid),
            4 => Some(FftBand::HighMid),
            5 => Some(FftBand::High),
            6 => Some(FftBand::VeryHigh),
            7 => Some(FftBand::Presence),
            _ => None,
        }
    }
}

/// Parameters that can be modulated by audio
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModulationTarget {
    /// Hue shift parameter.
    HueShift,
    /// Saturation parameter.
    Saturation,
    /// Brightness parameter.
    Brightness,
    /// Internal render width.
    InternalWidth,
    /// Internal render height.
    InternalHeight,
    /// Audio input gain.
    AudioAmplitude,
    /// Audio smoothing factor.
    AudioSmoothing,
    /// Input texture opacity.
    InputOpacity,
    /// Output texture opacity.
    OutputOpacity,
}

impl ModulationTarget {
    /// Human-readable target name.
    pub fn name(&self) -> &'static str {
        match self {
            ModulationTarget::HueShift => "Hue Shift",
            ModulationTarget::Saturation => "Saturation",
            ModulationTarget::Brightness => "Brightness",
            ModulationTarget::InternalWidth => "Internal Width",
            ModulationTarget::InternalHeight => "Internal Height",
            ModulationTarget::AudioAmplitude => "Audio Amplitude",
            ModulationTarget::AudioSmoothing => "Audio Smoothing",
            ModulationTarget::InputOpacity => "Input Opacity",
            ModulationTarget::OutputOpacity => "Output Opacity",
        }
    }
    
    /// All supported modulation targets.
    pub fn all() -> &'static [ModulationTarget] {
        &[
            ModulationTarget::HueShift,
            ModulationTarget::Saturation,
            ModulationTarget::Brightness,
            ModulationTarget::InternalWidth,
            ModulationTarget::InternalHeight,
            ModulationTarget::AudioAmplitude,
            ModulationTarget::AudioSmoothing,
            ModulationTarget::InputOpacity,
            ModulationTarget::OutputOpacity,
        ]
    }
}

/// A single audio-to-parameter routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioRoute {
    /// Unique ID for this route
    pub id: usize,
    /// Which FFT band to use
    pub band: FftBand,
    /// Which parameter to modulate
    pub target: ModulationTarget,
    /// Modulation depth (-1.0 to 1.0, can be bipolar)
    pub amount: f32,
    /// Attack smoothing (0.0 = instant, 1.0 = very slow)
    pub attack: f32,
    /// Release smoothing (0.0 = instant, 1.0 = very slow)
    pub release: f32,
    /// Whether this route is enabled
    pub enabled: bool,
    /// Current modulated value (runtime only, not serialized)
    #[serde(skip)]
    pub current_value: f32,
    /// Current smoothed FFT value (runtime only)
    #[serde(skip)]
    smoothed_fft: f32,
}

impl AudioRoute {
    /// Create a new audio route
    pub fn new(id: usize, band: FftBand, target: ModulationTarget) -> Self {
        Self {
            id,
            band,
            target,
            amount: 0.5,
            attack: 0.1,
            release: 0.3,
            enabled: true,
            current_value: 0.0,
            smoothed_fft: 0.0,
        }
    }
    
    /// Process this route with new FFT data
    /// 
    /// # Arguments
    /// * `fft_bands` - Array of 8 FFT band values (0.0 to 1.0)
    /// * `delta_time` - Time since last frame in seconds
    pub fn process(&mut self, fft_bands: &[f32; 8], delta_time: f32) {
        if !self.enabled {
            self.current_value = 0.0;
            self.smoothed_fft = self.smoothed_fft * 0.9; // Decay to 0
            return;
        }
        
        // Get current FFT value for our band
        let target_value = fft_bands[self.band as usize];
        
        // Apply attack/release smoothing
        let diff = target_value - self.smoothed_fft;
        let smoothing = if diff > 0.0 { self.attack } else { self.release };
        
        // Exponential smoothing
        let smoothing_factor = (-delta_time / smoothing.max(0.001)).exp();
        self.smoothed_fft = self.smoothed_fft * smoothing_factor + target_value * (1.0 - smoothing_factor);
        
        // Calculate modulation value
        self.current_value = self.smoothed_fft * self.amount;
    }
    
    /// Reset smoothed values
    pub fn reset(&mut self) {
        self.current_value = 0.0;
        self.smoothed_fft = 0.0;
    }
}

/// Manages all audio-to-parameter routings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingMatrix {
    routes: Vec<AudioRoute>,
    next_id: usize,
    max_routes: usize,
}

impl RoutingMatrix {
    /// Create a new routing matrix
    pub fn new(max_routes: usize) -> Self {
        Self {
            routes: Vec::new(),
            next_id: 0,
            max_routes,
        }
    }
    
    /// Create with default routes
    pub fn with_defaults() -> Self {
        let mut matrix = Self::new(8);
        
        // Add some default routes
        matrix.add_route(FftBand::Bass, ModulationTarget::Brightness);
        matrix.add_route(FftBand::High, ModulationTarget::Saturation);
        
        matrix
    }
    
    /// Add a new route
    /// 
    /// Returns the ID of the new route, or None if at max capacity
    pub fn add_route(&mut self, band: FftBand, target: ModulationTarget) -> Option<usize> {
        if self.routes.len() >= self.max_routes {
            return None;
        }
        
        let id = self.next_id;
        self.next_id += 1;
        
        self.routes.push(AudioRoute::new(id, band, target));
        Some(id)
    }
    
    /// Remove a route by ID
    pub fn remove_route(&mut self, id: usize) {
        self.routes.retain(|r| r.id != id);
    }
    
    /// Remove a route by index
    pub fn remove_route_at(&mut self, index: usize) {
        if index < self.routes.len() {
            self.routes.remove(index);
        }
    }
    
    /// Get a route by ID
    pub fn get_route(&self, id: usize) -> Option<&AudioRoute> {
        self.routes.iter().find(|r| r.id == id)
    }
    
    /// Get a mutable route by ID
    pub fn get_route_mut(&mut self, id: usize) -> Option<&mut AudioRoute> {
        self.routes.iter_mut().find(|r| r.id == id)
    }
    
    /// Get all routes
    pub fn routes(&self) -> &[AudioRoute] {
        &self.routes
    }
    
    /// Get mutable access to all routes
    pub fn routes_mut(&mut self) -> &mut [AudioRoute] {
        &mut self.routes
    }
    
    /// Process all routes with new FFT data
    pub fn process(&mut self, fft_bands: &[f32; 8], delta_time: f32) {
        for route in &mut self.routes {
            route.process(fft_bands, delta_time);
        }
    }
    
    /// Get the modulation value for a specific target
    /// 
    /// If multiple routes target the same parameter, their values are summed
    /// and clamped to a reasonable range.
    pub fn get_modulation(&self, target: ModulationTarget) -> f32 {
        let total: f32 = self.routes
            .iter()
            .filter(|r| r.target == target && r.enabled)
            .map(|r| r.current_value)
            .sum();
        
        // Clamp to reasonable range
        total.clamp(-2.0, 2.0)
    }
    
    /// Get all modulations as a map
    pub fn get_all_modulations(&self) -> HashMap<ModulationTarget, f32> {
        let mut map = HashMap::new();
        for target in ModulationTarget::all() {
            map.insert(*target, self.get_modulation(*target));
        }
        map
    }
    
    /// Apply modulations to HSB parameters
    /// 
    /// Returns the modulation offsets for hue, saturation, brightness
    pub fn apply_to_hsb(&self, base_hue: f32, base_sat: f32, base_bright: f32) -> (f32, f32, f32) {
        let hue_mod = self.get_modulation(ModulationTarget::HueShift);
        let sat_mod = self.get_modulation(ModulationTarget::Saturation);
        let bright_mod = self.get_modulation(ModulationTarget::Brightness);
        
        // Apply modulation with clamping
        let new_hue = (base_hue + hue_mod * 180.0).clamp(-180.0, 180.0);
        let new_sat = (base_sat + sat_mod * 2.0).clamp(0.0, 2.0);
        let new_bright = (base_bright + bright_mod * 2.0).clamp(0.0, 2.0);
        
        (new_hue, new_sat, new_bright)
    }
    
    /// Clear all routes
    pub fn clear(&mut self) {
        self.routes.clear();
    }
    
    /// Get number of routes
    pub fn len(&self) -> usize {
        self.routes.len()
    }
    
    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
    
    /// Get max routes
    pub fn max_routes(&self) -> usize {
        self.max_routes
    }
    
    /// Check if can add more routes
    pub fn can_add_route(&self) -> bool {
        self.routes.len() < self.max_routes
    }
    
    /// Reset all smoothed values
    pub fn reset(&mut self) {
        for route in &mut self.routes {
            route.reset();
        }
    }
}

impl Default for RoutingMatrix {
    fn default() -> Self {
        Self::new(8)
    }
}

/// Audio routing state for the app
#[derive(Debug, Serialize, Deserialize)]
pub struct AudioRoutingState {
    /// The routing matrix
    pub matrix: RoutingMatrix,
    /// Whether audio routing is enabled
    pub enabled: bool,
    /// Show routing window
    #[serde(skip)]
    pub show_window: bool,
    /// Selected band for new route
    #[serde(skip)]
    pub selected_band: usize,
    /// Selected target for new route
    #[serde(skip)]
    pub selected_target: usize,
    /// Base hue value (before modulation)
    pub base_hue: f32,
    /// Base saturation value (before modulation)
    pub base_saturation: f32,
    /// Base brightness value (before modulation)
    pub base_brightness: f32,
}

impl Default for AudioRoutingState {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioRoutingState {
    /// Create a new routing state with default routes and disabled modulation.
    pub fn new() -> Self {
        Self {
            matrix: RoutingMatrix::with_defaults(),
            enabled: false, // Disabled by default
            show_window: false,
            selected_band: 1, // Bass
            selected_target: 1, // Saturation
            base_hue: 0.0,
            base_saturation: 1.0,
            base_brightness: 1.0,
        }
    }
    
    /// Update base values from current HSB params (call when user changes values in UI)
    pub fn update_base_values(&mut self, hue: f32, saturation: f32, brightness: f32) {
        self.base_hue = hue;
        self.base_saturation = saturation;
        self.base_brightness = brightness;
    }
}
