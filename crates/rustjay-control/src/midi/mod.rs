//! # MIDI Integration with Learn System
//!
//! MIDI control change (CC) mapping with learn functionality.

use midir::{Ignore, MidiInput, MidiInputConnection};
use std::sync::{Arc, Mutex};

#[cfg(feature = "mtc")]
pub mod mtc;

/// Commands for MIDI device and learn-mode control
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiCommand {
    None,
    RefreshDevices,
    SelectDevice(String),
    StartLearn { param_path: String, param_name: String },
    CancelLearn,
    ClearMappings,
}

/// A mapped MIDI parameter
#[derive(Debug, Clone)]
pub struct MidiMapping {
    /// Controller number (0-127)
    pub cc: u8,
    /// MIDI channel (0-15)
    pub channel: u8,
    /// Human-readable parameter name
    pub name: String,
    /// Parameter path for OSC/address (e.g., "color/hue_shift")
    pub param_path: String,
    /// Current value (0.0 - 1.0)
    pub value: f32,
    /// Min output range
    pub min_value: f32,
    /// Max output range
    pub max_value: f32,
    /// Whether this value has been updated since last read
    pub dirty: bool,
}

impl MidiMapping {
    pub fn new(cc: u8, channel: u8, name: &str, param_path: &str, min: f32, max: f32) -> Self {
        Self {
            cc,
            channel,
            name: name.to_string(),
            param_path: param_path.to_string(),
            value: 0.0,
            min_value: min,
            max_value: max,
            dirty: false,
        }
    }

    /// Update value from MIDI CC value (0-127)
    pub fn update_from_midi(&mut self, midi_value: u8) {
        let normalized = midi_value as f32 / 127.0;
        let new_value = self.min_value + normalized * (self.max_value - self.min_value);
        if (new_value - self.value).abs() > 0.001 {
            self.value = new_value;
            self.dirty = true;
        }
    }

    /// Get the scaled value and clear dirty flag
    pub fn get_scaled_value(&mut self) -> f32 {
        self.dirty = false;
        self.value
    }

    /// Peek value without clearing dirty flag
    pub fn peek_value(&self) -> f32 {
        self.value
    }

    /// Check if value is dirty (has been updated)
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
}

/// Learn mode state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnState {
    /// Not in learn mode
    Idle,
    /// Waiting for MIDI input
    Waiting,
    /// Learned a CC, waiting for parameter selection
    Learned { cc: u8, channel: u8 },
}

/// MIDI controller input
#[derive(Debug, Clone, Copy)]
pub struct MidiInputEvent {
    pub channel: u8,
    pub cc: u8,
    pub value: u8,
}

/// Shared MIDI state
pub struct MidiState {
    /// All current mappings
    pub mappings: Vec<MidiMapping>,
    /// Current learn state
    pub learn_state: LearnState,
    /// Last received input (for debugging/display)
    pub last_input: Option<MidiInputEvent>,
    /// Currently selected device name
    pub selected_device: Option<String>,
    /// Available devices (updated on refresh)
    pub available_devices: Vec<String>,
    /// Whether MIDI is enabled
    pub enabled: bool,
    /// Parameter currently being learned (path)
    pub learning_param_path: Option<String>,
    /// Parameter name being learned
    pub learning_param_name: Option<String>,
}

impl Default for MidiState {
    fn default() -> Self {
        Self {
            mappings: Vec::new(),
            learn_state: LearnState::Idle,
            last_input: None,
            selected_device: None,
            available_devices: Vec::new(),
            enabled: false,
            learning_param_path: None,
            learning_param_name: None,
        }
    }
}

impl MidiState {
    /// Start learning a parameter
    pub fn start_learning(&mut self, param_path: &str, param_name: &str) {
        self.learn_state = LearnState::Waiting;
        self.learning_param_path = Some(param_path.to_string());
        self.learning_param_name = Some(param_name.to_string());
        log::info!("MIDI learn started for: {}", param_name);
    }

    /// Cancel learning
    pub fn cancel_learning(&mut self) {
        self.learn_state = LearnState::Idle;
        self.learning_param_path = None;
        self.learning_param_name = None;
        log::info!("MIDI learn cancelled");
    }

    /// Complete learning with received CC
    pub fn complete_learning(&mut self, cc: u8, channel: u8) {
        if let (Some(path), Some(name)) = (&self.learning_param_path, &self.learning_param_name) {
            self.mappings.retain(|m| !(m.cc == cc && m.channel == channel));
            let mapping = MidiMapping::new(cc, channel, name, path, 0.0, 1.0);
            self.mappings.push(mapping);
            log::info!("MIDI mapped: {} -> CC {} channel {}", name, cc, channel);
        }
        self.learn_state = LearnState::Idle;
        self.learning_param_path = None;
        self.learning_param_name = None;
    }

    /// Remove a mapping
    pub fn remove_mapping(&mut self, index: usize) {
        if index < self.mappings.len() {
            let mapping = self.mappings.remove(index);
            log::info!("Removed MIDI mapping: {}", mapping.name);
        }
    }

    /// Update mapping range
    pub fn update_mapping_range(&mut self, index: usize, min: f32, max: f32) {
        if let Some(mapping) = self.mappings.get_mut(index) {
            mapping.min_value = min;
            mapping.max_value = max;
        }
    }

    /// Handle incoming MIDI CC message
    pub fn handle_cc(&mut self, channel: u8, cc: u8, value: u8) {
        self.last_input = Some(MidiInputEvent { channel, cc, value });
        match self.learn_state {
            LearnState::Waiting => {
                self.complete_learning(cc, channel);
            }
            _ => {
                for mapping in &mut self.mappings {
                    if mapping.cc == cc && mapping.channel == channel {
                        mapping.update_from_midi(value);
                    }
                }
            }
        }
    }

    /// Get current value for a parameter path (peek without clearing dirty)
    pub fn get_value(&self, param_path: &str) -> Option<f32> {
        self.mappings
            .iter()
            .find(|m| m.param_path == param_path)
            .map(|m| m.peek_value())
    }

    /// Check if a parameter is currently mapped
    pub fn is_mapped(&self, param_path: &str) -> bool {
        self.mappings.iter().any(|m| m.param_path == param_path)
    }

    /// Get mapping for a parameter
    pub fn get_mapping(&self, param_path: &str) -> Option<&MidiMapping> {
        self.mappings.iter().find(|m| m.param_path == param_path)
    }
}

/// MIDI manager handling input connections
pub struct MidiManager {
    state: Arc<Mutex<MidiState>>,
    input: Option<MidiInput>,
    connection: Option<MidiInputConnection<()>>,
    /// Throttle the port-availability check to avoid creating a MidiInput every frame
    last_availability_check: std::time::Instant,
}

impl MidiManager {
    /// Create a new MIDI manager with the given shared state.
    pub fn new(state: Arc<Mutex<MidiState>>) -> anyhow::Result<Self> {
        let mut input = MidiInput::new("RustJay MIDI")?;
        input.ignore(Ignore::None);

        Ok(Self {
            state,
            input: Some(input),
            connection: None,
            last_availability_check: std::time::Instant::now(),
        })
    }

    /// Check if the connected device is still available in the port list.
    /// Only performs the actual check at most once every 3 seconds.
    pub fn check_device_available_if_needed(&mut self) -> Option<bool> {
        if self.last_availability_check.elapsed().as_secs() < 3 {
            return None;
        }
        self.last_availability_check = std::time::Instant::now();

        let device_name = match self.state.lock() {
            Ok(s) => s.selected_device.clone(),
            Err(_) => return Some(true),
        };

        let Some(name) = device_name else {
            return Some(true);
        };

        if let Ok(mut tmp) = MidiInput::new("RustJay MIDI Check") {
            tmp.ignore(Ignore::None);
            let available = tmp.ports().iter()
                .any(|p| tmp.port_name(p).ok().as_deref() == Some(name.as_str()));
            Some(available)
        } else {
            Some(true)
        }
    }

    /// Refresh list of available devices
    pub fn refresh_devices(&mut self) -> Vec<String> {
        if let Some(ref mut input) = self.input {
            let ports = input.ports();
            let device_names: Vec<String> = ports
                .iter()
                .filter_map(|p| input.port_name(p).ok())
                .collect();
            if let Ok(mut state) = self.state.lock() {
                state.available_devices = device_names.clone();
            }
            device_names
        } else {
            if let Ok(mut input) = MidiInput::new("RustJay MIDI") {
                input.ignore(Ignore::None);
                let ports = input.ports();
                let device_names: Vec<String> = ports
                    .iter()
                    .filter_map(|p| input.port_name(p).ok())
                    .collect();
                if let Ok(mut state) = self.state.lock() {
                    state.available_devices = device_names.clone();
                }
                self.input = Some(input);
                device_names
            } else {
                Vec::new()
            }
        }
    }

    /// Connect to a MIDI device by name
    pub fn connect(&mut self, device_name: &str) -> anyhow::Result<()> {
        self.disconnect();

        let input = self.input.take()
            .ok_or_else(|| anyhow::anyhow!("MIDI input not available"))?;

        let ports = input.ports();
        let port = ports
            .into_iter()
            .find(|p| input.port_name(p).map(|n| n == device_name).unwrap_or(false))
            .ok_or_else(|| anyhow::anyhow!("MIDI device '{}' not found", device_name))?;

        let state = Arc::clone(&self.state);

        let conn = input.connect(
            &port,
            "rustjay-midi",
            move |_stamp, message, _| {
                if message.is_empty() { return; }
                let status = message[0];
                // CC message: status 0xB0–0xBF
                if (status & 0xF0) == 0xB0 && message.len() >= 3 {
                    let channel = status & 0x0F;
                    let cc      = message[1];
                    let value   = message[2];
                    if let Ok(mut state) = state.lock() {
                        state.handle_cc(channel, cc, value);
                    }
                }
            },
            (),
        ).map_err(|e| anyhow::anyhow!("MIDI connect error: {}", e))?;

        self.connection = Some(conn);

        if let Ok(mut state) = self.state.lock() {
            state.selected_device = Some(device_name.to_string());
            state.enabled = true;
        }

        log::info!("Connected to MIDI device: {}", device_name);
        Ok(())
    }

    /// Disconnect from current device
    pub fn disconnect(&mut self) {
        if let Some(conn) = self.connection.take() {
            let _ = conn.close();
            log::info!("MIDI disconnected");
        }
        if self.input.is_none() {
            if let Ok(mut input) = MidiInput::new("RustJay MIDI") {
                input.ignore(Ignore::None);
                self.input = Some(input);
            }
        }
        if let Ok(mut state) = self.state.lock() {
            state.selected_device = None;
            state.enabled = false;
        }
    }

    /// Start learning a parameter
    pub fn start_learn(&mut self, param_path: &str, param_name: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.start_learning(param_path, param_name);
        }
    }

    /// Cancel learning
    pub fn cancel_learn(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.cancel_learning();
        }
    }

    /// Get shared state reference
    pub fn state(&self) -> Arc<Mutex<MidiState>> {
        Arc::clone(&self.state)
    }
}

impl Drop for MidiManager {
    fn drop(&mut self) {
        self.disconnect();
    }
}

/// Get list of available MIDI devices (without creating a manager)
pub fn list_midi_devices() -> Vec<String> {
    if let Ok(mut input) = MidiInput::new("RustJay MIDI List") {
        input.ignore(Ignore::None);
        let ports = input.ports();
        ports
            .iter()
            .filter_map(|p| input.port_name(p).ok())
            .collect()
    } else {
        Vec::new()
    }
}
