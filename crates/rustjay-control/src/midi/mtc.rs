//! MIDI Timecode (MTC) receive — auto-listens on all available MIDI ports.
//!
//! [`MtcReceiver`] opens an input connection on every MIDI port it finds and
//! reassembles the 8 quarter-frame messages that encode one SMPTE position.
//! It refreshes the port list periodically so devices plugged in after startup
//! (including IAC Driver virtual buses) are picked up automatically.

use midir::{Ignore, MidiInput, MidiInputConnection};
use rustjay_core::{MtcFrameRate, MtcState, SmpteTime};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// ── Startup epoch for last_qf_ms timestamps ───────────────────────────────

static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn now_ms() -> u64 {
    EPOCH.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

// ── Nibble reassembler (per-port, not shared) ─────────────────────────────

/// Reassembles MTC quarter-frame messages into SMPTE timecodes.
///
/// One instance lives inside each MIDI callback closure — never shared between
/// threads, so no locking is needed.
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
    fn parse_full_frame(msg: &[u8]) -> Option<SmpteTime> {
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

// ── Packed AtomicU64 layout ───────────────────────────────────────────────
//
//  bits [ 4: 0]  hours   (0–23,  5 bits)
//  bits [10: 5]  minutes (0–59,  6 bits)
//  bits [16:11]  seconds (0–59,  6 bits)
//  bits [21:17]  frames  (0–29,  5 bits)
//  bits [23:22]  rate    (0–3,   2 bits)  MtcFrameRate discriminant
//  bit  [   24]  running
//  bit  [   25]  playing

fn pack_smpte(tc: &SmpteTime, running: bool, playing: bool) -> u64 {
    let rate = match tc.frame_rate {
        MtcFrameRate::Fps24       => 0u64,
        MtcFrameRate::Fps25       => 1u64,
        MtcFrameRate::Fps2997Drop => 2u64,
        MtcFrameRate::Fps30       => 3u64,
    };
    (tc.hours as u64)
        | ((tc.minutes as u64) << 5)
        | ((tc.seconds as u64) << 11)
        | ((tc.frames  as u64) << 17)
        | (rate << 22)
        | ((running as u64) << 24)
        | ((playing as u64) << 25)
}

fn unpack_smpte(packed: u64) -> (SmpteTime, bool, bool) {
    let tc = SmpteTime {
        hours:      ( packed        & 0x1F) as u8,
        minutes:    ((packed >>  5) & 0x3F) as u8,
        seconds:    ((packed >> 11) & 0x3F) as u8,
        frames:     ((packed >> 17) & 0x1F) as u8,
        frame_rate: match (packed >> 22) & 0x03 {
            0 => MtcFrameRate::Fps24,
            1 => MtcFrameRate::Fps25,
            2 => MtcFrameRate::Fps2997Drop,
            _ => MtcFrameRate::Fps30,
        },
    };
    let running = ((packed >> 24) & 1) != 0;
    let playing = ((packed >> 25) & 1) != 0;
    (tc, running, playing)
}

// ── Lock-free published state ─────────────────────────────────────────────

/// Shared between the MIDI callback threads (writers) and the engine thread
/// (reader).  All hot-path fields are lock-free.
struct MtcPublished {
    /// Packed SMPTE position + running/playing flags (see layout above).
    smpte: AtomicU64,
    /// Milliseconds since process start when the last quarter-frame arrived.
    last_qf_ms: AtomicU64,
    /// Name of the MIDI port currently providing MTC.  Changes at most once
    /// per port-connect event, so a Mutex is fine — it is never contended in
    /// steady state.
    source_device: Mutex<String>,
}

impl MtcPublished {
    fn new() -> Self {
        Self {
            smpte:         AtomicU64::new(0),
            last_qf_ms:    AtomicU64::new(0),
            source_device: Mutex::new(String::new()),
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
    published:       Arc<MtcPublished>,
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
            published:       Arc::new(MtcPublished::new()),
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

            let published = Arc::clone(&self.published);
            let device    = name.clone();
            // Each port gets its own decoder — no sharing, no locking on the
            // hot path (240 quarter-frames/sec at 30 fps MTC).
            let mut decoder = MtcDecoder::new();

            let result = input.connect(
                &port,
                "rustjay-mtc",
                move |_, msg, _| {
                    if msg.is_empty() { return; }
                    match msg[0] {
                        0xF1 if msg.len() >= 2 => {
                            // Record arrival time before any decode work.
                            published.last_qf_ms.store(now_ms(), Ordering::Release);

                            if let Some(tc) = decoder.feed_quarter_frame(msg[1]) {
                                log::debug!("[MTC] {} from {}", tc, device);
                                // Update source device name — try_lock avoids any
                                // block if the reader happens to hold it.
                                if let Ok(mut src) = published.source_device.try_lock() {
                                    if src.as_str() != device {
                                        src.clear();
                                        src.push_str(&device);
                                    }
                                }
                                published.smpte.store(
                                    pack_smpte(&tc, true, true),
                                    Ordering::Release,
                                );
                            } else {
                                // Running but no complete SMPTE yet — set flags only.
                                published.smpte.fetch_or(
                                    (1u64 << 24) | (1u64 << 25),
                                    Ordering::Relaxed,
                                );
                            }
                        }
                        0xF0 if msg.len() >= 10
                            && msg[3] == 0x01 && msg[4] == 0x01 =>
                        {
                            if let Some(tc) = MtcDecoder::parse_full_frame(msg) {
                                log::info!("[MTC] Full-frame locate: {} from {}", tc, device);
                                if let Ok(mut src) = published.source_device.try_lock() {
                                    if src.as_str() != device {
                                        src.clear();
                                        src.push_str(&device);
                                    }
                                }
                                // Full-frame is a locate — running but not playing.
                                published.smpte.store(
                                    pack_smpte(&tc, true, false),
                                    Ordering::Release,
                                );
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

    /// Clear the `playing` flag if no quarter-frame has arrived in 500 ms.
    /// Call once per engine frame.
    pub fn tick(&self) {
        let last = self.published.last_qf_ms.load(Ordering::Acquire);
        if now_ms().saturating_sub(last) > 500 {
            // Atomically clear the playing bit (bit 25).
            self.published.smpte.fetch_and(!(1u64 << 25), Ordering::Relaxed);
        }
    }

    /// Snapshot the current MTC state.  Lock-free on the hot path.
    pub fn clone_state(&self) -> MtcState {
        let packed = self.published.smpte.load(Ordering::Acquire);
        let (position, running, playing) = unpack_smpte(packed);
        let source_device = self.published.source_device
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        MtcState { position, running, playing, source_device }
    }

    /// Port names currently being listened to.
    pub fn connected_ports(&self) -> &[String] {
        &self.connected_names
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smpte_roundtrip_exhaustive() {
        for hours   in 0u8..24 {
            for minutes in 0u8..60 {
                for seconds in 0u8..60 {
                    for frames in 0u8..30 {
                        for &rate in &[
                            MtcFrameRate::Fps24,
                            MtcFrameRate::Fps25,
                            MtcFrameRate::Fps2997Drop,
                            MtcFrameRate::Fps30,
                        ] {
                            let tc = SmpteTime { hours, minutes, seconds, frames, frame_rate: rate };
                            let packed = pack_smpte(&tc, true, false);
                            let (tc2, running, playing) = unpack_smpte(packed);
                            assert_eq!(tc2.hours,      hours,   "hours mismatch");
                            assert_eq!(tc2.minutes,    minutes, "minutes mismatch");
                            assert_eq!(tc2.seconds,    seconds, "seconds mismatch");
                            assert_eq!(tc2.frames,     frames,  "frames mismatch");
                            assert_eq!(tc2.frame_rate, rate,    "rate mismatch");
                            assert!(running);
                            assert!(!playing);
                        }
                    }
                }
            }
        }
    }
}
