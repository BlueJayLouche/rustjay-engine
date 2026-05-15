//! MIDI Timecode (MTC) receive — auto-listens on all available MIDI ports.
//!
//! [`MtcReceiver`] opens an input connection on every MIDI port it finds and
//! reassembles the 8 quarter-frame messages that encode one SMPTE position.
//! It refreshes the port list periodically so devices plugged in after startup
//! (including IAC Driver virtual buses) are picked up automatically.

use midir::{Ignore, MidiInput, MidiInputConnection};
use rustjay_core::{MtcFrameRate, MtcState, SmpteTime};
use std::sync::{Arc, Mutex};

// ── Nibble reassembler ────────────────────────────────────────────────────

/// Reassembles MTC quarter-frame messages into SMPTE timecodes.
///
/// MTC encodes one timecode value across 8 quarter-frame messages (types
/// 0–7). Each carries a 4-bit nibble. A bitmask tracks which types have
/// arrived; once all 8 are seen the full HH:MM:SS:FF position is assembled.
struct MtcDecoder {
    nibbles: [u8; 8],
    seen: u8,
}

impl MtcDecoder {
    fn new() -> Self {
        Self { nibbles: [0; 8], seen: 0 }
    }

    fn feed_quarter_frame(&mut self, data: u8) -> Option<SmpteTime> {
        let msg_type = ((data >> 4) & 0x07) as usize;
        self.nibbles[msg_type] = data & 0x0F;
        self.seen |= 1u8 << msg_type;
        if self.seen == 0xFF {
            self.seen = 0;
            Some(self.assemble())
        } else {
            None
        }
    }

    /// Full-frame SysEx: `F0 7F <dev> 01 01 hr mn sc fr F7`
    fn feed_full_frame(&self, msg: &[u8]) -> Option<SmpteTime> {
        if msg.len() < 10 || msg[3] != 0x01 || msg[4] != 0x01 { return None; }
        let hr = msg[5];
        Some(SmpteTime {
            hours:      hr & 0x1F,
            minutes:    msg[6],
            seconds:    msg[7],
            frames:     msg[8],
            frame_rate: Self::decode_rate((hr >> 5) & 0x03),
        })
    }

    fn assemble(&self) -> SmpteTime {
        SmpteTime {
            frames:     self.nibbles[0] | (self.nibbles[1] << 4),
            seconds:    self.nibbles[2] | (self.nibbles[3] << 4),
            minutes:    self.nibbles[4] | (self.nibbles[5] << 4),
            hours:      self.nibbles[6] | ((self.nibbles[7] & 0x01) << 4),
            frame_rate: Self::decode_rate((self.nibbles[7] >> 1) & 0x03),
        }
    }

    fn decode_rate(bits: u8) -> MtcFrameRate {
        match bits {
            0 => MtcFrameRate::Fps24,
            1 => MtcFrameRate::Fps25,
            2 => MtcFrameRate::Fps2997Drop,
            _ => MtcFrameRate::Fps30,
        }
    }
}

// ── Per-port receive state (lives behind the callback mutex) ──────────────

struct MtcRxState {
    decoder:  MtcDecoder,
    pub_state: MtcState,
    last_qf:  std::time::Instant,
}

impl MtcRxState {
    fn new() -> Self {
        Self {
            decoder:  MtcDecoder::new(),
            pub_state: MtcState::default(),
            last_qf:  std::time::Instant::now(),
        }
    }

    fn on_quarter_frame(&mut self, data: u8, device: &str) {
        self.last_qf = std::time::Instant::now();
        self.pub_state.running = true;
        self.pub_state.playing = true;
        if let Some(tc) = self.decoder.feed_quarter_frame(data) {
            log::debug!("[MTC] {} from {}", tc, device);
            self.pub_state.position    = tc;
            self.pub_state.source_device = device.to_string();
        }
    }

    fn on_full_frame(&mut self, msg: &[u8], device: &str) {
        self.pub_state.running = true;
        if let Some(tc) = self.decoder.feed_full_frame(msg) {
            log::info!("[MTC] Full-frame locate: {} from {}", tc, device);
            self.pub_state.position    = tc;
            self.pub_state.source_device = device.to_string();
            // Full-frame is a locate command; transport may not be rolling.
            self.pub_state.playing = false;
        }
    }

    /// Clear `playing` if no quarter-frame has arrived in 500 ms.
    fn tick(&mut self) {
        if self.pub_state.playing && self.last_qf.elapsed().as_millis() > 500 {
            self.pub_state.playing = false;
        }
    }
}

// ── Public receiver ───────────────────────────────────────────────────────

/// Listens for MIDI Timecode on **all** available MIDI input ports at once.
///
/// Created once at startup; call [`refresh`](MtcReceiver::refresh) each frame
/// (internally throttled to once per 5 s) to pick up devices plugged in after
/// launch. The decoded [`MtcState`] is available via [`clone_state`](MtcReceiver::clone_state).
pub struct MtcReceiver {
    rx_state:        Arc<Mutex<MtcRxState>>,
    /// Port names we have successfully connected to.
    connected_names: Vec<String>,
    /// Live connections — dropping one closes the port.
    connections:     Vec<MidiInputConnection<()>>,
    last_refresh:    std::time::Instant,
}

impl MtcReceiver {
    /// Create a receiver and immediately connect to all currently visible ports.
    pub fn new() -> Self {
        let mut r = Self {
            rx_state:        Arc::new(Mutex::new(MtcRxState::new())),
            connected_names: Vec::new(),
            connections:     Vec::new(),
            // Make elapsed() > 5 s so the first refresh() call runs immediately.
            last_refresh:    std::time::Instant::now()
                - std::time::Duration::from_secs(10),
        };
        r.refresh();
        r
    }

    /// Scan for MIDI ports not yet connected and open them.
    ///
    /// Internally throttled: exits immediately if called again within 5 s.
    pub fn refresh(&mut self) {
        if self.last_refresh.elapsed().as_secs() < 5 { return; }
        self.last_refresh = std::time::Instant::now();

        // Probe: list all port names with a throw-away MidiInput.
        let new_names = {
            let Ok(mut probe) = MidiInput::new("RustJay MTC Probe") else { return };
            probe.ignore(Ignore::None);
            probe.ports()
                .iter()
                .filter_map(|p| probe.port_name(p).ok())
                .filter(|n| !self.connected_names.contains(n))
                .collect::<Vec<_>>()
        };

        for name in new_names {
            // Each connection needs its own MidiInput.
            let Ok(mut input) = MidiInput::new(&format!("RustJay MTC [{}]", &name)) else {
                continue;
            };
            input.ignore(Ignore::None);

            let ports = input.ports();
            let Some(port) = ports
                .iter()
                .find(|p| input.port_name(p).ok().as_deref() == Some(name.as_str()))
                .cloned()
            else {
                continue;
            };

            let state  = Arc::clone(&self.rx_state);
            let device = name.clone();

            let result = input.connect(
                &port,
                "rustjay-mtc",
                move |_, msg, _| {
                    if msg.is_empty() { return; }
                    match msg[0] {
                        0xF1 if msg.len() >= 2 => {
                            if let Ok(mut s) = state.lock() {
                                s.on_quarter_frame(msg[1], &device);
                            }
                        }
                        0xF0 if msg.len() >= 10
                            && msg[3] == 0x01 && msg[4] == 0x01 =>
                        {
                            if let Ok(mut s) = state.lock() {
                                s.on_full_frame(msg, &device);
                            }
                        }
                        _ => {}
                    }
                },
                (),
            );

            match result {
                Ok(conn) => {
                    log::info!("[MTC] Listening on: {}", name);
                    self.connections.push(conn);
                    self.connected_names.push(name);
                }
                Err(e) => log::warn!("[MTC] Failed to open {}: {}", name, e),
            }
        }
    }

    /// Age out the `playing` flag if no quarter-frame has arrived in 500 ms.
    /// Call once per engine frame.
    pub fn tick(&self) {
        if let Ok(mut s) = self.rx_state.lock() { s.tick(); }
    }

    /// Snapshot the current MTC state.
    pub fn clone_state(&self) -> MtcState {
        self.rx_state.lock()
            .map(|s| s.pub_state.clone())
            .unwrap_or_default()
    }

    /// Port names currently being listened to.
    pub fn connected_ports(&self) -> &[String] {
        &self.connected_names
    }
}
