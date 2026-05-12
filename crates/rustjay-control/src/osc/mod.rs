//! # OSC Integration
//!
//! UDP-based OSC server with auto-generated addresses.
//! Address format: /[base]/[tab]/[parameter]

/// Commands for OSC server control
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscCommand {
    None,
    Start,
    Stop,
    SetPort(u16),
    RefreshAddresses,
}

use rosc::{OscPacket, OscMessage, OscType, decoder};
use std::collections::HashMap;
use std::net::{UdpSocket, SocketAddrV4, Ipv4Addr};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::Duration;

/// OSC parameter descriptor
#[derive(Debug, Clone)]
pub struct OscParameter {
    /// Full OSC address (e.g., "/rustjay/color/hue_shift")
    pub address: String,
    /// Human-readable name
    pub name: String,
    /// Current value
    pub value: f32,
    /// Min value for range
    pub min_value: f32,
    /// Max value for range
    pub max_value: f32,
    /// Parameter type/category (for grouping)
    pub category: String,
    /// Whether this value has been updated since last read
    pub dirty: bool,
}

impl OscParameter {
    pub fn new(address: &str, name: &str, category: &str, min: f32, max: f32) -> Self {
        Self {
            address: address.to_string(),
            name: name.to_string(),
            value: 0.0,
            min_value: min,
            max_value: max,
            category: category.to_string(),
            dirty: false,
        }
    }
    
    /// Set value from normalized OSC input (0.0 - 1.0)
    pub fn set_normalized(&mut self, normalized: f32) {
        let new_value = self.min_value + normalized.clamp(0.0, 1.0) * (self.max_value - self.min_value);
        if (new_value - self.value).abs() > 0.001 {
            self.value = new_value;
            self.dirty = true;
        }
    }
    
    /// Get normalized value (0.0 - 1.0)
    pub fn get_normalized(&self) -> f32 {
        if self.max_value > self.min_value {
            (self.value - self.min_value) / (self.max_value - self.min_value)
        } else {
            0.0
        }
    }
    
    /// Set absolute value (clamped to range)
    pub fn set_value(&mut self, value: f32) {
        let new_value = value.clamp(self.min_value, self.max_value);
        if (new_value - self.value).abs() > 0.001 {
            self.value = new_value;
            self.dirty = true;
        }
    }
    
    /// Get value and clear dirty flag
    pub fn get_value(&mut self) -> f32 {
        self.dirty = false;
        self.value
    }
    
    /// Check if value is dirty
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
}

/// OSC server state
pub struct OscState {
    /// All registered parameters by address
    pub parameters: HashMap<String, OscParameter>,
    /// Whether server is running
    pub running: bool,
    /// Port number
    pub port: u16,
    /// Base address prefix
    pub base_address: String,
    /// Last received message (for debugging)
    pub last_message: Option<(String, f32)>,
    /// Message history (recent messages)
    pub message_log: Vec<(String, f32, f64)>,
}

impl OscState {
    pub fn new(port: u16, base_address: &str) -> Self {
        let base = if base_address.starts_with('/') {
            base_address.to_string()
        } else {
            format!("/{}", base_address)
        };
        
        Self {
            parameters: HashMap::new(),
            running: false,
            port,
            base_address: base,
            last_message: None,
            message_log: Vec::with_capacity(100),
        }
    }
    
    /// Register a parameter
    pub fn register_parameter(&mut self, address: &str, name: &str, category: &str, min: f32, max: f32) {
        let full_address = format!("{}{}", self.base_address, address);
        let param = OscParameter::new(&full_address, name, category, min, max);
        self.parameters.insert(full_address.clone(), param);
        log::debug!("Registered OSC parameter: {}", full_address);
    }
    
    /// Auto-register parameters based on the application structure
    pub fn register_default_parameters(&mut self) {
        // Color/HSB parameters
        self.register_parameter("/color/hue_shift", "Hue Shift", "color", -180.0, 180.0);
        self.register_parameter("/color/saturation", "Saturation", "color", 0.0, 2.0);
        self.register_parameter("/color/brightness", "Brightness", "color", 0.0, 2.0);
        self.register_parameter("/color/enabled", "Color Enabled", "color", 0.0, 1.0);
        
        // Audio parameters
        self.register_parameter("/audio/amplitude", "Audio Amplitude", "audio", 0.0, 5.0);
        self.register_parameter("/audio/smoothing", "Audio Smoothing", "audio", 0.0, 1.0);
        self.register_parameter("/audio/enabled", "Audio Enabled", "audio", 0.0, 1.0);
        self.register_parameter("/audio/normalize", "Normalize", "audio", 0.0, 1.0);
        self.register_parameter("/audio/pink_noise", "Pink Noise", "audio", 0.0, 1.0);
        
        // Output parameters
        self.register_parameter("/output/width", "Output Width", "output", 320.0, 4096.0);
        self.register_parameter("/output/height", "Output Height", "output", 240.0, 2160.0);
        self.register_parameter("/output/fullscreen", "Fullscreen", "output", 0.0, 1.0);
        
        // Resolution parameters
        self.register_parameter("/resolution/internal_width", "Internal Width", "resolution", 320.0, 4096.0);
        self.register_parameter("/resolution/internal_height", "Internal Height", "resolution", 240.0, 2160.0);
    }
    
    /// Update parameter value from OSC input
    pub fn update_parameter(&mut self, address: &str, value: f32) {
        let full_address = if address.starts_with(&self.base_address) {
            address.to_string()
        } else {
            format!("{}{}", self.base_address, address)
        };
        
        if let Some(param) = self.parameters.get_mut(&full_address) {
            param.set_normalized(value.clamp(0.0, 1.0));
            self.last_message = Some((full_address.clone(), value));
            
            // Add to log
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            self.message_log.push((full_address, value, now));
            
            // Keep log size manageable
            if self.message_log.len() > 100 {
                self.message_log.remove(0);
            }
        }
    }
    
    /// Get parameter value (peek without clearing dirty)
    pub fn get_value(&self, address: &str) -> Option<f32> {
        let full_address = if address.starts_with(&self.base_address) {
            address.to_string()
        } else {
            format!("{}{}", self.base_address, address)
        };
        
        self.parameters.get(&full_address).map(|p| p.value)
    }
    
    /// Get parameter value and clear dirty flag (for reading OSC updates)
    pub fn get_value_if_dirty(&mut self, address: &str) -> Option<f32> {
        let full_address = if address.starts_with(&self.base_address) {
            address.to_string()
        } else {
            format!("{}{}", self.base_address, address)
        };
        
        if let Some(param) = self.parameters.get_mut(&full_address) {
            if param.is_dirty() {
                return Some(param.get_value());
            }
        }
        None
    }
    
    /// Set parameter value (from UI) - doesn't mark as dirty
    pub fn set_value(&mut self, address: &str, value: f32) {
        let full_address = if address.starts_with(&self.base_address) {
            address.to_string()
        } else {
            format!("{}{}", self.base_address, address)
        };
        
        if let Some(param) = self.parameters.get_mut(&full_address) {
            param.value = value.clamp(param.min_value, param.max_value);
            // Note: We don't set dirty here since this is from UI, not OSC
        }
    }
    
    /// Check if parameter exists
    pub fn has_parameter(&self, address: &str) -> bool {
        let full_address = if address.starts_with(&self.base_address) {
            address.to_string()
        } else {
            format!("{}{}", self.base_address, address)
        };
        
        self.parameters.contains_key(&full_address)
    }
    
    /// Clear message log
    pub fn clear_log(&mut self) {
        self.message_log.clear();
        self.last_message = None;
    }
}

/// OSC Server handling UDP input
pub struct OscServer {
    state: Arc<Mutex<OscState>>,
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl OscServer {
    pub fn new(port: u16, base_address: &str) -> Self {
        let state = Arc::new(Mutex::new(OscState::new(port, base_address)));
        let running = Arc::new(AtomicBool::new(false));
        
        Self {
            state,
            running,
            handle: None,
        }
    }
    
    /// Get shared state
    pub fn state(&self) -> Arc<Mutex<OscState>> {
        Arc::clone(&self.state)
    }
    
    /// Start the OSC server
    pub fn start(&mut self) -> anyhow::Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }
        
        let port = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.port
        };
        
        // Create socket
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        
        log::info!("OSC server started on port {}", port);
        
        self.running.store(true, Ordering::SeqCst);
        let running = Arc::clone(&self.running);
        let state = Arc::clone(&self.state);
        
        let handle = thread::spawn(move || {
            let mut buf = [0u8; 1536];
            
            while running.load(Ordering::SeqCst) {
                // Try to receive a packet
                match socket.recv_from(&mut buf) {
                    Ok((size, _)) => {
                        // Parse OSC packet
                        match decoder::decode_udp(&buf[..size]) {
                            Ok((_, packet)) => {
                                Self::handle_packet(&state, &packet);
                            }
                            Err(e) => {
                                log::warn!("OSC decode error: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        if e.kind() != std::io::ErrorKind::WouldBlock {
                            log::warn!("OSC receive error: {}", e);
                        }
                        // Small sleep to prevent busy-waiting
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            }
            
            log::info!("OSC server thread stopped");
        });
        
        self.handle = Some(handle);
        
        // Mark as running
        if let Ok(mut state) = self.state.lock() {
            state.running = true;
        }
        
        Ok(())
    }
    
    /// Stop the OSC server
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        
        if let Ok(mut state) = self.state.lock() {
            state.running = false;
        }
        
        log::info!("OSC server stopped");
    }
    
    /// Handle an OSC packet
    fn handle_packet(state: &Arc<Mutex<OscState>>, packet: &OscPacket) {
        match packet {
            OscPacket::Message(msg) => {
                Self::handle_message(state, msg);
            }
            OscPacket::Bundle(bundle) => {
                for content in &bundle.content {
                    Self::handle_packet(state, content);
                }
            }
        }
    }
    
    /// Handle an OSC message
    fn handle_message(state: &Arc<Mutex<OscState>>, msg: &OscMessage) {
        // Extract value from arguments
        let value = msg.args.first().and_then(|arg| match arg {
            OscType::Float(f) => Some(*f),
            OscType::Double(d) => Some(*d as f32),
            OscType::Int(i) => Some(*i as f32 / 127.0), // Normalize MIDI-style int
            OscType::Long(l) => Some(*l as f32),
            _ => None,
        });
        
        if let Some(v) = value {
            if let Ok(mut state) = state.lock() {
                state.update_parameter(&msg.addr, v);
                log::debug!("OSC: {} = {}", msg.addr, v);
            }
        }
    }
    
    /// Check if server is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

impl Drop for OscServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Helper to generate OSC address from components
pub fn make_address(base: &str, tab: &str, param: &str) -> String {
    format!("{}/{}/{}", base.trim_end_matches('/'), tab, param)
}

/// Helper to format address for display
pub fn format_address_for_display(address: &str) -> String {
    address.trim_start_matches('/').replace('/', " → ")
}