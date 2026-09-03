//! # Audio Routing System
//!
//! Routes audio FFT bands to various parameters for audio-reactive visuals.
//! Adapted from rustjay-delta for HSB color parameters.

use crate::params::{ParamType, ParameterDescriptor};
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

    /// Frequency range covered by this band, in Hz.
    pub fn freq_range(&self) -> (f32, f32) {
        match self {
            FftBand::SubBass => (20.0, 60.0),
            FftBand::Bass => (60.0, 120.0),
            FftBand::LowMid => (120.0, 250.0),
            FftBand::Mid => (250.0, 500.0),
            FftBand::HighMid => (500.0, 2000.0),
            FftBand::High => (2000.0, 4000.0),
            FftBand::VeryHigh => (4000.0, 8000.0),
            FftBand::Presence => (8000.0, 16000.0),
        }
    }
}

/// Parameters that can be modulated by audio.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// Modulate an effect-declared custom parameter.
    Custom(String),
    /// Unrecognised variant from an older preset file.
    /// The route is preserved in the preset bank but has no effect.
    #[serde(other)]
    Unknown,
}

impl ModulationTarget {
    /// Human-readable target name.
    pub fn name(&self) -> String {
        match self {
            ModulationTarget::HueShift => "Hue Shift".to_string(),
            ModulationTarget::Saturation => "Saturation".to_string(),
            ModulationTarget::Brightness => "Brightness".to_string(),
            ModulationTarget::InternalWidth => "Internal Width".to_string(),
            ModulationTarget::InternalHeight => "Internal Height".to_string(),
            ModulationTarget::AudioAmplitude => "Audio Amplitude".to_string(),
            ModulationTarget::AudioSmoothing => "Audio Smoothing".to_string(),
            ModulationTarget::InputOpacity => "Input Opacity".to_string(),
            ModulationTarget::OutputOpacity => "Output Opacity".to_string(),
            ModulationTarget::Custom(id) => id.clone(),
            ModulationTarget::Unknown => "(unknown)".to_string(),
        }
    }

    /// All static modulation targets (excludes `Unknown`).
    /// For backward compatibility.
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

    /// Generate the full list of modulation targets for a set of descriptors.
    pub fn all_for(descriptors: &[ParameterDescriptor]) -> Vec<ModulationTarget> {
        let mut targets: Vec<ModulationTarget> = Self::all().to_vec();
        for d in descriptors {
            if matches!(d.param_type, ParamType::Float | ParamType::Int) {
                targets.push(ModulationTarget::Custom(d.id.clone()));
            }
        }
        targets
    }

    /// Get the parameter id for this target (if it's a parameter target).
    pub fn param_id(&self) -> Option<&str> {
        match self {
            ModulationTarget::HueShift => Some("hue_shift"),
            ModulationTarget::Saturation => Some("saturation"),
            ModulationTarget::Brightness => Some("brightness"),
            ModulationTarget::InternalWidth => Some("internal_width"),
            ModulationTarget::InternalHeight => Some("internal_height"),
            ModulationTarget::AudioAmplitude => Some("audio_amplitude"),
            ModulationTarget::AudioSmoothing => Some("audio_smoothing"),
            ModulationTarget::InputOpacity => Some("input_opacity"),
            ModulationTarget::OutputOpacity => Some("output_opacity"),
            ModulationTarget::Custom(id) => Some(id),
            ModulationTarget::Unknown => None,
        }
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
            self.smoothed_fft *= 0.9; // Decay to 0
            return;
        }

        // Get current FFT value for our band
        let target_value = fft_bands[self.band as usize];

        // Apply attack/release smoothing
        let diff = target_value - self.smoothed_fft;
        let smoothing = if diff > 0.0 {
            self.attack
        } else {
            self.release
        };

        // Exponential smoothing
        let dt = delta_time.max(0.0);
        let smoothing = smoothing.max(0.001);
        if !dt.is_finite() || !smoothing.is_finite() {
            return;
        }
        let smoothing_factor = (-dt / smoothing).exp();
        self.smoothed_fft =
            self.smoothed_fft * smoothing_factor + target_value * (1.0 - smoothing_factor);

        // Calculate modulation value
        self.current_value = self.smoothed_fft * self.amount;
    }

    /// Reset smoothed values
    pub fn reset(&mut self) {
        self.current_value = 0.0;
        self.smoothed_fft = 0.0;
    }
}

/// How many audio routes a matrix will hold.
///
/// A sanity bound, not a structural one — routes live in a `Vec` and are
/// applied on the CPU, so this only exists to stop a runaway UI. It was a
/// serialised per-matrix field fixed at 8, which meant raising it did nothing
/// for anyone with a saved preset; as a constant, old presets pick up the new
/// ceiling and their leftover `max_routes` key is simply ignored.
pub const MAX_ROUTES: usize = 64;

/// Manages all audio-to-parameter routings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingMatrix {
    routes: Vec<AudioRoute>,
    next_id: usize,
}

impl RoutingMatrix {
    /// Create a new routing matrix
    pub fn new() -> Self {
        Self {
            routes: Vec::new(),
            next_id: 0,
        }
    }

    /// Create with default routes
    pub fn with_defaults() -> Self {
        let mut matrix = Self::new();

        // Add some default routes
        matrix.add_route(FftBand::Bass, ModulationTarget::Brightness);
        matrix.add_route(FftBand::High, ModulationTarget::Saturation);

        matrix
    }

    /// Add a new route
    ///
    /// Returns the ID of the new route, or None if at max capacity
    pub fn add_route(&mut self, band: FftBand, target: ModulationTarget) -> Option<usize> {
        if self.routes.len() >= MAX_ROUTES {
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
        let total: f32 = self
            .routes
            .iter()
            .filter(|r| r.target == target && r.enabled)
            .map(|r| r.current_value)
            .sum();

        // Clamp to reasonable range
        total.clamp(-2.0, 2.0)
    }

    /// Like `get_modulation` but accepts a plain string id for `Custom` targets.
    /// Avoids the `String` allocation in `ModulationTarget::Custom(id.clone())` on hot paths.
    pub fn get_modulation_for_str(&self, id: &str) -> f32 {
        let total: f32 = self
            .routes
            .iter()
            .filter(|r| r.enabled && matches!(&r.target, ModulationTarget::Custom(s) if s == id))
            .map(|r| r.current_value)
            .sum();
        total.clamp(-2.0, 2.0)
    }

    /// Get all modulations as a map for the static targets.
    pub fn get_all_modulations(&self) -> HashMap<ModulationTarget, f32> {
        let mut map = HashMap::new();
        for target in ModulationTarget::all() {
            let value = self.get_modulation(target.clone());
            map.insert(target.clone(), value);
        }
        map
    }

    /// Get all modulations as a map of `param_id → value` for dynamic targets.
    pub fn get_all_modulations_for(
        &self,
        descriptors: &[ParameterDescriptor],
    ) -> HashMap<String, f32> {
        let mut map = HashMap::new();
        for target in ModulationTarget::all_for(descriptors) {
            let id = target.param_id().map(|s| s.to_string());
            let value = self.get_modulation(target);
            if let Some(id) = id {
                map.insert(id, value);
            }
        }
        map
    }

    /// Apply modulations to HSB parameters.
    #[deprecated(note = "Use `apply_to_params` for generic parameter support.")]
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

    /// Apply modulations to a parameter slice.
    /// Reads base values from `bases`, applies audio routing modulations,
    /// and writes modulated values into `params`.
    pub fn apply_to_params(
        &self,
        params: &mut [f32],
        bases: &[f32],
        descriptors: &[ParameterDescriptor],
    ) {
        for (i, desc) in descriptors.iter().enumerate() {
            if !desc.is_modulatable() {
                continue;
            }
            let mod_value = self.get_modulation_for_str(&desc.id);
            let base = bases[i];
            let range = desc.max - desc.min;
            params[i] = if range > 0.0 {
                (base + mod_value * range).clamp(desc.min, desc.max)
            } else {
                base
            };
        }
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
        MAX_ROUTES
    }

    /// Check if can add more routes
    pub fn can_add_route(&self) -> bool {
        self.routes.len() < MAX_ROUTES
    }

    /// Reset all smoothed values
    pub fn reset(&mut self) {
        for route in &mut self.routes {
            route.reset();
        }
    }

    /// Convert routes into modulation source entries.
    ///
    /// Each route becomes an [`crate::modulation::ModulationSource::AudioBand`]
    /// entry with frequency bounds derived from the route's [`FftBand`].
    /// Disabled routes migrate as disabled sources (rather than being
    /// dropped) so they survive the move and can be re-enabled from the
    /// routing window.
    pub fn to_modulation_sources(&self) -> Vec<crate::modulation::ModulationSourceEntry> {
        use crate::modulation::{ModulationSource, ModulationSourceEntry};
        self.routes
            .iter()
            .map(|route| {
                let (freq_low, freq_high) = route.band.freq_range();
                let source = ModulationSource::AudioBand {
                    source_id: None,
                    freq_low,
                    freq_high,
                    gain: 1.0,
                    smoothing: route.release,
                    attack: route.attack,
                    enabled: route.enabled,
                    mode: crate::modulation::AudioReactMode::Direct,
                    noise_gate: 0.1,
                };
                ModulationSourceEntry::with_uuid(format!("route_{}", route.id), source)
            })
            .collect()
    }

    /// Convert this matrix into a full [`crate::modulation::ModulationEngine`].
    pub fn to_modulation_engine(&self) -> crate::modulation::ModulationEngine {
        use crate::modulation::{ModulationEngine, ModulationSource};
        let mut engine = ModulationEngine::new();
        for route in &self.routes {
            let (freq_low, freq_high) = route.band.freq_range();
            let source = ModulationSource::AudioBand {
                source_id: None,
                freq_low,
                freq_high,
                gain: 1.0,
                smoothing: route.release,
                attack: route.attack,
                enabled: route.enabled,
                mode: crate::modulation::AudioReactMode::Direct,
                noise_gate: 0.1,
            };
            let uuid = engine.add_source_with_uuid(format!("route_{}", route.id), source);
            if let Some(param_id) = route.target.param_id() {
                engine.assign(param_id, &uuid, route.amount, None);
            }
        }
        engine
    }
}

impl Default for RoutingMatrix {
    fn default() -> Self {
        Self::new()
    }
}

/// Audio routing state for the app
#[derive(Debug, Clone, Serialize, Deserialize)]
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
            selected_band: 1,   // Bass
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

    /// Convert the saved routing matrix into a modulation engine snapshot for
    /// merging into `EngineState::modulation` on load.
    ///
    /// The global `enabled` switch has no equivalent in the modulation engine,
    /// so a routing state saved with routing switched off migrates its sources
    /// disabled — preserving the saved behaviour. They can be re-enabled
    /// individually from the routing window.
    pub fn to_modulation_engine(&self) -> crate::modulation::ModulationEngine {
        let mut engine = self.matrix.to_modulation_engine();
        if !self.enabled {
            for entry in &mut engine.sources {
                if let crate::modulation::ModulationSource::AudioBand { enabled, .. } =
                    &mut entry.source
                {
                    *enabled = false;
                }
            }
        }
        engine
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matrix_holds_more_than_the_old_eight_routes() {
        let mut matrix = RoutingMatrix::new();
        for _ in 0..12 {
            assert!(
                matrix.add_route(FftBand::Bass, ModulationTarget::Brightness).is_some(),
                "a route below the cap must be accepted"
            );
        }
        assert_eq!(matrix.len(), 12);
        assert_eq!(matrix.max_routes(), MAX_ROUTES);
    }

    #[test]
    fn the_cap_still_holds_at_its_new_ceiling() {
        let mut matrix = RoutingMatrix::new();
        for _ in 0..MAX_ROUTES {
            matrix.add_route(FftBand::Bass, ModulationTarget::Brightness);
        }
        assert!(!matrix.can_add_route());
        assert!(matrix.add_route(FftBand::Bass, ModulationTarget::Brightness).is_none());
    }

    /// A preset written when `max_routes` was a serialised field still loads,
    /// and picks up the new ceiling rather than staying pinned at 8.
    #[test]
    fn an_old_preset_is_not_stuck_at_eight() {
        let json = r#"{"routes":[],"next_id":0,"max_routes":8}"#;
        let matrix: RoutingMatrix = serde_json::from_str(json).expect("old preset loads");
        assert_eq!(matrix.max_routes(), MAX_ROUTES);
        assert!(matrix.can_add_route());
    }

    // ── U2: migration into the modulation engine ─────────────────────

    fn two_route_matrix() -> RoutingMatrix {
        let mut matrix = RoutingMatrix::new();
        matrix.add_route(FftBand::Bass, ModulationTarget::Brightness);
        matrix.add_route(FftBand::High, ModulationTarget::Custom("spin".to_string()));
        matrix
    }

    #[test]
    fn a_matrix_migrates_to_matching_sources_and_assignments() {
        let engine = two_route_matrix().to_modulation_engine();
        assert_eq!(engine.sources.len(), 2);
        assert!(engine.has_source("route_0"));
        assert!(engine.has_source("route_1"));
        assert_eq!(
            engine.assignments.values().map(Vec::len).sum::<usize>(),
            2
        );
        assert!(engine.has_modulation("brightness"));
        assert!(engine.has_modulation("spin"));
    }

    #[test]
    fn migrating_twice_updates_in_place_instead_of_duplicating() {
        let mut engine = crate::modulation::ModulationEngine::new();
        engine.merge(two_route_matrix().to_modulation_engine());
        engine.merge(two_route_matrix().to_modulation_engine());
        assert_eq!(engine.sources.len(), 2, "second load must not duplicate");
        assert_eq!(
            engine.assignments.values().map(Vec::len).sum::<usize>(),
            2,
            "assignments must not duplicate either"
        );
    }

    #[test]
    fn re_merging_picks_up_changed_route_values() {
        let mut engine = crate::modulation::ModulationEngine::new();
        engine.merge(two_route_matrix().to_modulation_engine());

        let mut matrix = two_route_matrix();
        matrix.get_route_mut(0).unwrap().amount = 0.9;
        engine.merge(matrix.to_modulation_engine());

        let mods = &engine.assignments["brightness"];
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].amount, 0.9, "the assignment is updated in place");
    }

    #[test]
    fn a_routes_attack_and_release_survive_the_trip() {
        let mut matrix = RoutingMatrix::new();
        let id = matrix
            .add_route(FftBand::Bass, ModulationTarget::Brightness)
            .unwrap();
        let route = matrix.get_route_mut(id).unwrap();
        route.attack = 0.7;
        route.release = 0.2;

        let engine = matrix.to_modulation_engine();
        let entry = engine.find_source_by_uuid("route_0").expect("migrated source");
        match &entry.source {
            crate::modulation::ModulationSource::AudioBand {
                attack, smoothing, ..
            } => {
                assert_eq!(*attack, 0.7);
                assert_eq!(*smoothing, 0.2, "release maps to smoothing");
            }
            other => panic!("expected AudioBand, got {other:?}"),
        }
    }

    #[test]
    fn a_disabled_routing_state_migrates_its_sources_disabled() {
        // AudioRoutingState::default() has enabled = false.
        let routing = AudioRoutingState {
            matrix: two_route_matrix(),
            ..AudioRoutingState::default()
        };
        let engine = routing.to_modulation_engine();
        assert_eq!(engine.sources.len(), 2);
        for entry in &engine.sources {
            match &entry.source {
                crate::modulation::ModulationSource::AudioBand { enabled, .. } => {
                    assert!(!enabled, "global disable must fold into the sources")
                }
                other => panic!("expected AudioBand, got {other:?}"),
            }
        }
    }
}
