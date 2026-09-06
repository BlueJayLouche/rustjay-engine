//! # OSC Integration
//!
//! UDP-based OSC server with auto-generated addresses.
//! Address format: /[base]/[tab]/[parameter]

/// Commands for OSC server control
// Superseded by `rustjay_core::OscCommand`; kept as the control-layer's own descriptor.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscCommand {
    None,
    Start,
    Stop,
    SetPort(u16),
    RefreshAddresses,
}

use rosc::{decoder, OscMessage, OscPacket, OscType};
#[cfg(feature = "osc-feedback")]
use rosc::encoder;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
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
        let new_value =
            self.min_value + normalized.clamp(0.0, 1.0) * (self.max_value - self.min_value);
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
    /// Host to bind to
    pub host: String,
    /// Port number
    pub port: u16,
    /// Base address prefix
    pub base_address: String,
    /// Last received message (for debugging)
    pub last_message: Option<(String, f32)>,
    /// Message history (recent messages)
    pub message_log: Vec<(String, f32, f64)>,
    /// String arguments, which are not parameters: `(address, text)`, drained
    /// by the host each frame. A text layer's copy arrives this way.
    pub text_inbox: Vec<(String, String)>,
    /// Controller feedback target, adopted from the last `<base>/sync` sender
    #[cfg(feature = "osc-feedback")]
    feedback: Option<(UdpSocket, std::net::SocketAddr)>,
}

impl OscState {
    pub fn new(host: &str, port: u16, base_address: &str) -> Self {
        let base = if base_address.starts_with('/') {
            base_address.to_string()
        } else {
            format!("/{}", base_address)
        };

        Self {
            parameters: HashMap::new(),
            running: false,
            host: host.to_string(),
            port,
            base_address: base,
            last_message: None,
            message_log: Vec::with_capacity(100),
            text_inbox: Vec::new(),
            #[cfg(feature = "osc-feedback")]
            feedback: None,
        }
    }

    /// Register a parameter
    pub fn register_parameter(
        &mut self,
        address: &str,
        name: &str,
        category: &str,
        min: f32,
        max: f32,
    ) {
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
        self.register_parameter(
            "/resolution/internal_width",
            "Internal Width",
            "resolution",
            320.0,
            4096.0,
        );
        self.register_parameter(
            "/resolution/internal_height",
            "Internal Height",
            "resolution",
            240.0,
            2160.0,
        );
    }

    /// Register effect-declared parameters dynamically.
    pub fn register_parameters(&mut self, descriptors: &[rustjay_core::ParameterDescriptor]) {
        for d in descriptors {
            let category = d.category.name().to_lowercase();
            let address = format!("/{}/{}", category, d.id);
            self.register_parameter(&address, &d.name, &category, d.min, d.max);
        }
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

    /// Get parameter value and clear dirty flag (for reading OSC updates).
    /// `address` must be the full OSC address (e.g. `"/rustjay/color/hue_shift"`).
    pub fn get_value_if_dirty(&mut self, address: &str) -> Option<f32> {
        if let Some(param) = self.parameters.get_mut(address)
            && param.is_dirty() {
                return Some(param.get_value());
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
            let new_value = value.clamp(param.min_value, param.max_value);
            let _changed = (new_value - param.value).abs() > 0.001;
            param.value = new_value;
            // Note: We don't set dirty here since this is from UI, not OSC
            #[cfg(feature = "osc-feedback")]
            if _changed {
                let normalized = self.parameters[&full_address].get_normalized();
                self.send_feedback(&full_address, normalized);
            }
        }
    }

    /// Adopt `target` as the controller feedback destination (last `/sync`
    /// sender wins).
    #[cfg(feature = "osc-feedback")]
    pub fn set_feedback_target(&mut self, target: std::net::SocketAddr) {
        match UdpSocket::bind("0.0.0.0:0") {
            Ok(sock) => {
                log::info!("OSC feedback -> {}", target);
                self.feedback = Some((sock, target));
            }
            Err(e) => log::warn!("OSC feedback bind failed: {}", e),
        }
    }

    /// Push every registered parameter to the feedback target (controller
    /// boot sync). ponytail: one datagram per param, no bundling — revisit if
    /// the controller drops part of the burst.
    #[cfg(feature = "osc-feedback")]
    pub fn send_all_feedback(&self) {
        for p in self.parameters.values() {
            self.send_feedback(&p.address, p.get_normalized());
        }
    }

    #[cfg(feature = "osc-feedback")]
    fn send_feedback(&self, address: &str, normalized: f32) {
        if let Some((sock, target)) = &self.feedback
            && let Ok(bytes) = encoder::encode(&OscPacket::Message(OscMessage {
                addr: address.to_string(),
                args: vec![OscType::Float(normalized)],
            })) {
                let _ = sock.send_to(&bytes, target);
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
    /// Create a new OSC server (not started until [`start`](Self::start) is called).
    pub fn new(host: &str, port: u16, base_address: &str) -> Self {
        let state = Arc::new(Mutex::new(OscState::new(host, port, base_address)));
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

        // Create socket bound to configured host (default: 127.0.0.1)
        let host = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.host.clone()
        };
        let bind_addr: Ipv4Addr = host.parse().unwrap_or(Ipv4Addr::LOCALHOST);
        if !bind_addr.is_loopback() {
            log::warn!(
                "OSC server binding to non-loopback address {}. OSC has no authentication — \
                 anyone on the network can send control messages.",
                bind_addr
            );
        }
        let addr = SocketAddrV4::new(bind_addr, port);
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
                    Ok((size, peer)) => {
                        // Parse OSC packet
                        match decoder::decode_udp(&buf[..size]) {
                            Ok((_, packet)) => {
                                Self::handle_packet(&state, &packet, peer.ip());
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
    fn handle_packet(state: &Arc<Mutex<OscState>>, packet: &OscPacket, peer: std::net::IpAddr) {
        match packet {
            OscPacket::Message(msg) => {
                Self::handle_message(state, msg, peer);
            }
            OscPacket::Bundle(bundle) => {
                for content in &bundle.content {
                    Self::handle_packet(state, content, peer);
                }
            }
        }
    }

    /// Handle an OSC message
    fn handle_message(state: &Arc<Mutex<OscState>>, msg: &OscMessage, _peer: std::net::IpAddr) {
        // `<base>/sync [port]` — adopt the sender as feedback target and push
        // every current value (controller boot handshake). Default port 9001
        // is the Mk1 bridge's OSC input.
        #[cfg(feature = "osc-feedback")]
        if msg.addr.ends_with("/sync")
            && let Ok(mut st) = state.lock()
            && msg.addr == format!("{}/sync", st.base_address) {
                let port = msg.args.first().and_then(|a| match a {
                    OscType::Int(p) => u16::try_from(*p).ok(),
                    _ => None,
                });
                st.set_feedback_target((_peer, port.unwrap_or(9001)).into());
                st.send_all_feedback();
                return;
            }

        // A string argument is not a parameter — park it for the host to read.
        if let Some(OscType::String(text)) = msg.args.first()
            && let Ok(mut state) = state.lock()
        {
            // Bounded: a controller left sending into a host that never drains
            // must not grow this without limit.
            if state.text_inbox.len() >= 32 {
                state.text_inbox.remove(0);
            }
            state.text_inbox.push((msg.addr.clone(), text.clone()));
            log::debug!("OSC: {} = {:?}", msg.addr, text);
            return;
        }

        // Extract value from arguments
        let value = msg.args.first().and_then(|arg| match arg {
            OscType::Float(f) => Some(*f),
            OscType::Double(d) => Some(*d as f32),
            OscType::Int(i) => Some(*i as f32 / 127.0), // Normalize MIDI-style int
            OscType::Long(l) => Some(*l as f32),
            _ => None,
        });

        if let Some(v) = value
            && let Ok(mut state) = state.lock() {
                state.update_parameter(&msg.addr, v);
                log::debug!("OSC: {} = {}", msg.addr, v);
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
#[allow(dead_code)]
pub fn make_address(base: &str, tab: &str, param: &str) -> String {
    format!("{}/{}/{}", base.trim_end_matches('/'), tab, param)
}

/// Helper to format address for display
#[allow(dead_code)]
pub fn format_address_for_display(address: &str) -> String {
    address.trim_start_matches('/').replace('/', " → ")
}

#[cfg(all(test, feature = "osc-feedback"))]
mod feedback_tests {
    use super::*;

    #[test]
    fn set_value_feeds_back_normalized() {
        let controller = UdpSocket::bind("127.0.0.1:0").unwrap();
        controller
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        let mut state = OscState::new("127.0.0.1", 0, "/rustjay");
        state.register_parameter("/color/hue_shift", "Hue", "color", -180.0, 180.0);
        state.set_feedback_target(controller.local_addr().unwrap());

        state.set_value("/rustjay/color/hue_shift", 90.0);

        let mut buf = [0u8; 256];
        let (n, _) = controller.recv_from(&mut buf).expect("no feedback datagram");
        let (_, packet) = decoder::decode_udp(&buf[..n]).unwrap();
        let OscPacket::Message(msg) = packet else {
            panic!("expected message")
        };
        assert_eq!(msg.addr, "/rustjay/color/hue_shift");
        let OscType::Float(v) = msg.args[0] else {
            panic!("expected float")
        };
        assert!((v - 0.75).abs() < 1e-3, "got {v}"); // 90 in [-180,180] = 0.75

        // Same value again: delta guard must suppress the echo.
        state.set_value("/rustjay/color/hue_shift", 90.0);
        controller
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        assert!(controller.recv_from(&mut buf).is_err(), "unexpected echo");
    }
}
