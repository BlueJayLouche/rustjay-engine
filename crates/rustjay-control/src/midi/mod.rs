//! # MIDI Integration with Learn System
//!
//! CC, Note, and Aftertouch mapping with learn functionality.

// Protocol/learn-state field docs are self-evident from their names.
#![allow(missing_docs)]

use midir::{Ignore, MidiInput, MidiInputConnection};
use rustjay_core::MidiMsgKind;
use std::sync::{Arc, Mutex};

#[cfg(feature = "mtc")]
pub mod mtc;

/// Commands for MIDI device and learn-mode control
// Superseded by `rustjay_core::MidiCommand`; kept as the control-layer's own descriptor.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiCommand {
    None,
    RefreshDevices,
    SelectDevice(String),
    StartLearn {
        param_path: String,
        param_name: String,
    },
    CancelLearn,
    ClearMappings,
}

/// A mapped MIDI parameter
#[derive(Debug, Clone)]
pub struct MidiMapping {
    /// Message type (CC, Note, Aftertouch)
    pub kind: MidiMsgKind,
    /// Selector byte: CC number for CC, note number for Note, 0 for Aftertouch
    pub selector: u8,
    /// MIDI channel (0-15)
    pub channel: u8,
    /// Human-readable parameter name
    pub name: String,
    /// Parameter path for OSC/address (e.g., "color/hue_shift")
    pub param_path: String,
    /// Current value
    pub value: f32,
    /// Min output range
    pub min_value: f32,
    /// Max output range
    pub max_value: f32,
    /// Whether this value has been updated since last read
    pub dirty: bool,
}

impl MidiMapping {
    pub fn new(
        kind: MidiMsgKind,
        selector: u8,
        channel: u8,
        name: &str,
        param_path: &str,
        min: f32,
        max: f32,
    ) -> Self {
        Self {
            kind,
            selector,
            channel,
            name: name.to_string(),
            param_path: param_path.to_string(),
            value: min,
            min_value: min,
            max_value: max,
            dirty: false,
        }
    }

    /// Map a raw MIDI value (0–127) to the parameter range and mark dirty if changed.
    pub fn update_from_midi(&mut self, midi_value: u8) {
        let normalized = midi_value as f32 / 127.0;
        let new_value = self.min_value + normalized * (self.max_value - self.min_value);
        if (new_value - self.value).abs() > 0.001 {
            self.value = new_value;
            self.dirty = true;
        }
    }

    /// Drive the mapping to its minimum value (used for Note Off).
    pub fn drive_to_min(&mut self) {
        if (self.value - self.min_value).abs() > 0.001 {
            self.value = self.min_value;
            self.dirty = true;
        }
    }

    /// Drive the mapping to its maximum value (used for Note On — a note acts
    /// as a button, ignoring velocity so soft hits still trigger).
    pub fn drive_to_max(&mut self) {
        if (self.value - self.max_value).abs() > 0.001 {
            self.value = self.max_value;
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
    /// Waiting for any MIDI input
    Waiting,
    /// A message was captured; kept for potential future two-step UI flows
    Learned {
        kind: MidiMsgKind,
        selector: u8,
        channel: u8,
    },
}

/// A single received MIDI event (for debugging / last-input display)
#[derive(Debug, Clone, Copy)]
pub struct MidiInputEvent {
    pub kind: MidiMsgKind,
    pub channel: u8,
    /// CC number, note number, or 0 for channel aftertouch
    pub selector: u8,
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
    /// Minimum output value for the parameter being learned
    pub learning_param_min: f32,
    /// Maximum output value for the parameter being learned
    pub learning_param_max: f32,
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
            learning_param_min: 0.0,
            learning_param_max: 1.0,
        }
    }
}

impl MidiState {
    /// Start learning a parameter
    pub fn start_learning(&mut self, param_path: &str, param_name: &str, min: f32, max: f32) {
        self.learn_state = LearnState::Waiting;
        self.learning_param_path = Some(param_path.to_string());
        self.learning_param_name = Some(param_name.to_string());
        self.learning_param_min = min;
        self.learning_param_max = max;
        log::info!(
            "MIDI learn started for: {} (range {:.3}–{:.3})",
            param_name,
            min,
            max
        );
    }

    /// Cancel learning
    pub fn cancel_learning(&mut self) {
        self.learn_state = LearnState::Idle;
        self.learning_param_path = None;
        self.learning_param_name = None;
        log::info!("MIDI learn cancelled");
    }

    /// Complete learning with the received message
    pub fn complete_learning(&mut self, kind: MidiMsgKind, selector: u8, channel: u8) {
        if let (Some(path), Some(name)) = (&self.learning_param_path, &self.learning_param_name) {
            // Drop any prior mapping that used this same control (one control →
            // one param) AND any prior mapping for this param (re-learning
            // overwrites instead of stacking a duplicate).
            self.mappings.retain(|m| {
                !(m.kind == kind && m.selector == selector && m.channel == channel)
                    && m.param_path != *path
            });
            let mapping = MidiMapping::new(
                kind,
                selector,
                channel,
                name,
                path,
                self.learning_param_min,
                self.learning_param_max,
            );
            self.mappings.push(mapping);
            log::info!(
                "MIDI mapped: {} -> {:?} {} ch{} (range {:.3}–{:.3})",
                name,
                kind,
                selector,
                channel,
                self.learning_param_min,
                self.learning_param_max
            );
        }
        self.learn_state = LearnState::Idle;
        self.learning_param_path = None;
        self.learning_param_name = None;
    }

    /// Handle any incoming MIDI message, routing to learn or playback.
    ///
    /// `selector` is the CC number for CC messages, the note number for Note messages,
    /// and 0 for channel aftertouch.
    /// `value` is 0 to signal Note Off (either a real 0x80 or Note On with velocity 0).
    pub fn handle_message(&mut self, kind: MidiMsgKind, channel: u8, selector: u8, value: u8) {
        self.last_input = Some(MidiInputEvent {
            kind,
            channel,
            selector,
            value,
        });

        match self.learn_state {
            LearnState::Waiting => {
                // Don't learn on Note Off — wait for a Note On with velocity > 0.
                if kind == MidiMsgKind::Note && value == 0 {
                    return;
                }
                self.complete_learning(kind, selector, channel);
            }
            _ => {
                for mapping in &mut self.mappings {
                    if mapping.kind != kind
                        || mapping.selector != selector
                        || mapping.channel != channel
                    {
                        continue;
                    }
                    match kind {
                        // Notes act as buttons: any Note On drives the param to
                        // its max (so velocity-sensitive pads still trigger on
                        // soft hits), Note Off drives to min. CC/Aftertouch scale
                        // continuously.
                        MidiMsgKind::Note if value == 0 => mapping.drive_to_min(),
                        MidiMsgKind::Note => mapping.drive_to_max(),
                        _ => mapping.update_from_midi(value),
                    }
                }
            }
        }
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
            let available = tmp
                .ports()
                .iter()
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

        let input = self
            .input
            .take()
            .ok_or_else(|| anyhow::anyhow!("MIDI input not available"))?;

        let ports = input.ports();
        let port = ports
            .into_iter()
            .find(|p| {
                input
                    .port_name(p)
                    .map(|n| n == device_name)
                    .unwrap_or(false)
            })
            .ok_or_else(|| anyhow::anyhow!("MIDI device '{}' not found", device_name))?;

        let state = Arc::clone(&self.state);

        let conn = input
            .connect(
                &port,
                "rustjay-midi",
                move |_stamp, message, _| {
                    if message.is_empty() {
                        return;
                    }
                    let status = message[0];
                    let kind_byte = status & 0xF0;
                    let channel = status & 0x0F;

                    let msg = match kind_byte {
                        // CC
                        0xB0 if message.len() >= 3 => {
                            Some((MidiMsgKind::Cc, channel, message[1], message[2]))
                        }
                        // Note On — velocity 0 is treated as Note Off
                        0x90 if message.len() >= 3 => {
                            Some((MidiMsgKind::Note, channel, message[1], message[2]))
                        }
                        // Note Off — forward as Note with value 0
                        0x80 if message.len() >= 3 => {
                            Some((MidiMsgKind::Note, channel, message[1], 0))
                        }
                        // Channel (mono) Aftertouch
                        0xD0 if message.len() >= 2 => {
                            Some((MidiMsgKind::Aftertouch, channel, 0, message[1]))
                        }
                        _ => None,
                    };

                    if let Some((kind, ch, sel, val)) = msg
                        && let Ok(mut state) = state.lock() {
                            state.handle_message(kind, ch, sel, val);
                        }
                },
                (),
            )
            .map_err(|e| anyhow::anyhow!("MIDI connect error: {}", e))?;

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
        if self.input.is_none()
            && let Ok(mut input) = MidiInput::new("RustJay MIDI") {
                input.ignore(Ignore::None);
                self.input = Some(input);
            }
        if let Ok(mut state) = self.state.lock() {
            state.selected_device = None;
            state.enabled = false;
        }
    }

    /// Start learning a parameter
    pub fn start_learn(&mut self, param_path: &str, param_name: &str, min: f32, max: f32) {
        if let Ok(mut state) = self.state.lock() {
            state.start_learning(param_path, param_name, min, max);
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
#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A learned Note must drive the param to its max on *any* velocity (so
    /// velocity-sensitive pads still trigger on soft hits) and to min on Note Off.
    #[test]
    fn note_acts_as_button_ignoring_velocity() {
        let mut state = MidiState::default();
        state.start_learning("pads/pad0_trig", "Pad 1 Trigger", 0.0, 1.0);
        state.handle_message(MidiMsgKind::Note, 0, 48, 100); // learn on this note
        assert_eq!(state.mappings.len(), 1);

        // Soft hit (velocity 1) must still reach max (1.0), well above the 0.5
        // trigger threshold the host uses.
        state.handle_message(MidiMsgKind::Note, 0, 48, 1);
        assert_eq!(state.mappings[0].peek_value(), 1.0);

        // Note Off drives back to min.
        state.handle_message(MidiMsgKind::Note, 0, 48, 0);
        assert_eq!(state.mappings[0].peek_value(), 0.0);
    }

    /// Re-learning a param overwrites its mapping instead of stacking duplicates.
    #[test]
    fn relearn_overwrites_mapping() {
        let mut state = MidiState::default();
        state.start_learning("pads/pad0_trig", "Pad 1 Trigger", 0.0, 1.0);
        state.handle_message(MidiMsgKind::Note, 0, 48, 100);

        state.start_learning("pads/pad0_trig", "Pad 1 Trigger", 0.0, 1.0);
        state.handle_message(MidiMsgKind::Note, 0, 51, 100);

        assert_eq!(state.mappings.len(), 1, "re-learn should replace, not append");
        assert_eq!(state.mappings[0].selector, 51);
    }
}
