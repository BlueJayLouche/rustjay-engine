//! MIDI Timecode (MTC) receive — auto-listens on all available MIDI ports.
//!
//! [`MtcReceiver`] opens an input connection on every MIDI port it finds and
//! reassembles the 8 quarter-frame messages that encode one SMPTE position.
//! It refreshes the port list periodically so devices plugged in after startup
//! (including RTP-MIDI network sessions) are picked up automatically.

// ponytail: vendored copy of rustjay-control's MtcReceiver to avoid pulling
// axum/tokio/tower-http across the nested-workspace seam. Upgrade path: make
// rustjay-control's web deps optional and share the receiver.

use midir::{Ignore, MidiInput, MidiInputConnection};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// ── Local MTC types (stand-ins for rustjay_core::{SmpteTime, MtcFrameRate, MtcState}) ──

/// MTC frame rate, as encoded in the 2 rate bits of the hour nibble/byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MtcFrameRate {
    Fps24,
    #[default]
    Fps25,
    Fps2997Drop,
    Fps30,
}

impl MtcFrameRate {
    /// Nominal frames per second as a float.
    pub fn fps(self) -> f32 {
        match self {
            MtcFrameRate::Fps24 => 24.0,
            MtcFrameRate::Fps25 => 25.0,
            MtcFrameRate::Fps2997Drop => 29.97,
            MtcFrameRate::Fps30 => 30.0,
        }
    }

    /// Short human-readable label.
    pub fn name(self) -> &'static str {
        match self {
            MtcFrameRate::Fps24 => "24fps",
            MtcFrameRate::Fps25 => "25fps",
            MtcFrameRate::Fps2997Drop => "29.97fps DF",
            MtcFrameRate::Fps30 => "30fps",
        }
    }
}

/// A SMPTE HH:MM:SS:FF timecode position.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SmpteTime {
    /// Hours component (0–23).
    pub hours: u8,
    /// Minutes component (0–59).
    pub minutes: u8,
    /// Seconds component (0–59).
    pub seconds: u8,
    /// Frames component (0 to fps-1).
    pub frames: u8,
    /// Frame rate reported by the MTC source.
    pub frame_rate: MtcFrameRate,
}

impl SmpteTime {
    /// Timecode as fractional elapsed seconds.
    ///
    /// For 29.97 drop-frame the label is converted through its frame number
    /// rather than by reading H:M:S as seconds: labels 00 and 01 are skipped
    /// at the start of every minute except every tenth, and the true rate is
    /// 30000/1001.
    ///
    /// This is a precision fix, not a correctness one. Drop-frame numbering
    /// exists precisely so the label tracks wall clock, so reading it as
    /// seconds was already close — 3.6 ms out at 01:00:00:00 and 87 ms at
    /// 23:59:59:29, never near the 40 ms chase deadband at show-length
    /// timecodes. Converting through the frame number makes it exact.
    pub fn as_seconds_f64(self) -> f64 {
        if self.frame_rate == MtcFrameRate::Fps2997Drop {
            let total_minutes = self.hours as u64 * 60 + self.minutes as u64;
            let dropped = 2 * (total_minutes - total_minutes / 10);
            let frame_number =
                (self.hours as u64 * 3600 + self.minutes as u64 * 60 + self.seconds as u64) * 30
                    + self.frames as u64
                    - dropped;
            return frame_number as f64 / (30000.0 / 1001.0);
        }
        let fps = self.frame_rate.fps() as f64;
        self.hours as f64 * 3600.0
            + self.minutes as f64 * 60.0
            + self.seconds as f64
            + self.frames as f64 / fps
    }
}

impl std::fmt::Display for SmpteTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:02}:{:02}:{:02}:{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

/// Live state of MIDI Timecode (MTC) receive.
#[derive(Debug, Clone, Default)]
pub struct MtcState {
    /// `true` once any MTC message has been received on any MIDI port.
    pub running: bool,
    /// `true` while quarter-frame messages are arriving (transport playing/shuttling).
    pub playing: bool,
    /// Most recently assembled SMPTE timecode position.
    pub position: SmpteTime,
    /// Name of the MIDI port currently sending MTC (empty string if none yet).
    pub source_device: String,
}

// ── Startup epoch for last_qf_ms timestamps ───────────────────────────────

static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn now_ms() -> u64 {
    EPOCH
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
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
        Self {
            nibbles: [0; 8],
            seen: 0,
        }
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
        if msg.len() < 10 || msg[3] != 0x01 || msg[4] != 0x01 {
            return None;
        }
        let hr = msg[5];
        Some(SmpteTime {
            hours: hr & 0x1F,
            minutes: msg[6],
            seconds: msg[7],
            frames: msg[8],
            frame_rate: Self::decode_rate((hr >> 5) & 0x03),
        })
    }

    fn assemble(&self) -> SmpteTime {
        SmpteTime {
            frames: self.nibbles[0] | (self.nibbles[1] << 4),
            seconds: self.nibbles[2] | (self.nibbles[3] << 4),
            minutes: self.nibbles[4] | (self.nibbles[5] << 4),
            hours: self.nibbles[6] | ((self.nibbles[7] & 0x01) << 4),
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
        MtcFrameRate::Fps24 => 0u64,
        MtcFrameRate::Fps25 => 1u64,
        MtcFrameRate::Fps2997Drop => 2u64,
        MtcFrameRate::Fps30 => 3u64,
    };
    (tc.hours as u64)
        | ((tc.minutes as u64) << 5)
        | ((tc.seconds as u64) << 11)
        | ((tc.frames as u64) << 17)
        | (rate << 22)
        | ((running as u64) << 24)
        | ((playing as u64) << 25)
}

fn unpack_smpte(packed: u64) -> (SmpteTime, bool, bool) {
    let tc = SmpteTime {
        hours: (packed & 0x1F) as u8,
        minutes: ((packed >> 5) & 0x3F) as u8,
        seconds: ((packed >> 11) & 0x3F) as u8,
        frames: ((packed >> 17) & 0x1F) as u8,
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
            smpte: AtomicU64::new(0),
            last_qf_ms: AtomicU64::new(0),
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
    published: Arc<MtcPublished>,
    /// Port names we have successfully connected to.
    connected_names: Vec<String>,
    /// Live connections — dropping one closes the port.
    connections: Vec<MidiInputConnection<()>>,
    last_refresh: std::time::Instant,
}

impl MtcReceiver {
    /// Create a receiver and immediately connect to all currently visible ports.
    pub fn new() -> Self {
        let mut r = Self {
            published: Arc::new(MtcPublished::new()),
            connected_names: Vec::new(),
            connections: Vec::new(),
            // Make elapsed() > 5 s so the first refresh() call runs immediately.
            last_refresh: std::time::Instant::now() - std::time::Duration::from_secs(10),
        };
        r.refresh();
        r
    }

    /// Scan for MIDI ports not yet connected and open them.
    ///
    /// Internally throttled: exits immediately if called again within 5 s.
    pub fn refresh(&mut self) {
        if self.last_refresh.elapsed().as_secs() < 5 {
            return;
        }
        self.last_refresh = std::time::Instant::now();

        // Probe: list all port names with a throw-away MidiInput.
        let new_names = {
            let Ok(mut probe) = MidiInput::new("CuePool MTC Probe") else {
                return;
            };
            probe.ignore(Ignore::None);
            probe
                .ports()
                .iter()
                .filter_map(|p| probe.port_name(p).ok())
                .filter(|n| !self.connected_names.contains(n))
                .collect::<Vec<_>>()
        };

        for name in new_names {
            // Each connection needs its own MidiInput.
            let Ok(mut input) = MidiInput::new(&format!("CuePool MTC [{name}]")) else {
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
            let device = name.clone();
            // Each port gets its own decoder — no sharing, no locking on the
            // hot path (200 quarter-frames/sec at 25 fps MTC).
            let mut decoder = MtcDecoder::new();

            let result = input.connect(
                &port,
                "cuepool-mtc",
                move |_, msg, _| {
                    if msg.is_empty() {
                        return;
                    }
                    match msg[0] {
                        0xF1 if msg.len() >= 2 => {
                            // Record arrival time before any decode work.
                            published.last_qf_ms.store(now_ms(), Ordering::Release);

                            if let Some(tc) = decoder.feed_quarter_frame(msg[1]) {
                                log::debug!("[MTC] {} from {}", tc, device);
                                // Update source device name — try_lock avoids any
                                // block if the reader happens to hold it.
                                if let Ok(mut src) = published.source_device.try_lock()
                                    && src.as_str() != device
                                {
                                    src.clear();
                                    src.push_str(&device);
                                }
                                published
                                    .smpte
                                    .store(pack_smpte(&tc, true, true), Ordering::Release);
                            } else {
                                // Running but no complete SMPTE yet — set flags only.
                                published
                                    .smpte
                                    .fetch_or((1u64 << 24) | (1u64 << 25), Ordering::Relaxed);
                            }
                        }
                        0xF0 if msg.len() >= 10 && msg[3] == 0x01 && msg[4] != 0x01 => {
                            if let Some(tc) = MtcDecoder::parse_full_frame(msg) {
                                log::info!("[MTC] Full-frame locate: {} from {}", tc, device);
                                if let Ok(mut src) = published.source_device.try_lock()
                                    && src.as_str() != device
                                {
                                    src.clear();
                                    src.push_str(&device);
                                }
                                // Full-frame is a locate — running but not playing.
                                published
                                    .smpte
                                    .store(pack_smpte(&tc, true, false), Ordering::Release);
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
            self.published
                .smpte
                .fetch_and(!(1u64 << 25), Ordering::Relaxed);
        }
    }

    /// Snapshot the current MTC state.  Lock-free on the hot path.
    pub fn clone_state(&self) -> MtcState {
        let packed = self.published.smpte.load(Ordering::Acquire);
        let (position, running, playing) = unpack_smpte(packed);
        let source_device = self
            .published
            .source_device
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        MtcState {
            position,
            running,
            playing,
            source_device,
        }
    }

    /// Port names currently being listened to.
    pub fn connected_ports(&self) -> &[String] {
        &self.connected_names
    }
}

impl Default for MtcReceiver {
    fn default() -> Self {
        Self::new()
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smpte_roundtrip_exhaustive() {
        for hours in 0u8..24 {
            for minutes in 0u8..60 {
                for seconds in 0u8..60 {
                    for frames in 0u8..30 {
                        for &rate in &[
                            MtcFrameRate::Fps24,
                            MtcFrameRate::Fps25,
                            MtcFrameRate::Fps2997Drop,
                            MtcFrameRate::Fps30,
                        ] {
                            let tc = SmpteTime {
                                hours,
                                minutes,
                                seconds,
                                frames,
                                frame_rate: rate,
                            };
                            let packed = pack_smpte(&tc, true, false);
                            let (tc2, running, playing) = unpack_smpte(packed);
                            assert_eq!(tc2.hours, hours, "hours mismatch");
                            assert_eq!(tc2.minutes, minutes, "minutes mismatch");
                            assert_eq!(tc2.seconds, seconds, "seconds mismatch");
                            assert_eq!(tc2.frames, frames, "frames mismatch");
                            assert_eq!(tc2.frame_rate, rate, "rate mismatch");
                            assert!(running);
                            assert!(!playing);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn quarter_frame_reassembly_25fps() {
        // 01:00:00:00 @ 25fps, standard nibble order 0..7.
        let mut dec = MtcDecoder::new();
        // Nibbles: frames lsn/msn, seconds lsn/msn, minutes lsn/msn,
        // hours lsn, msn = rate(2 bits)<<1 | hours msb. 25fps = rate bits 01.
        let nibbles = [0x00, 0x10, 0x20, 0x30, 0x40, 0x50, 0x61, 0x72];
        let mut out = None;
        for n in nibbles {
            out = dec.feed_quarter_frame(n);
        }
        let tc = out.expect("complete quarter-frame sequence should assemble");
        assert_eq!(tc.hours, 1);
        assert_eq!(tc.minutes, 0);
        assert_eq!(tc.seconds, 0);
        assert_eq!(tc.frames, 0);
        assert_eq!(tc.frame_rate, MtcFrameRate::Fps25);
        assert_eq!(tc.as_seconds_f64(), 3600.0);
    }

    #[test]
    fn full_frame_parse_25fps() {
        // F0 7F 7F 01 01 hr mn sc fr F7 — hr bits 5-6 = rate (01 = 25fps).
        let msg = [0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x21, 0x0C, 0x22, 0x04, 0xF7];
        let tc = MtcDecoder::parse_full_frame(&msg).expect("valid full frame");
        assert_eq!(tc.hours, 1);
        assert_eq!(tc.minutes, 12);
        assert_eq!(tc.seconds, 34);
        assert_eq!(tc.frames, 4);
        assert_eq!(tc.frame_rate, MtcFrameRate::Fps25);
        assert_eq!(format!("{}", tc), "01:12:34:04");
    }

    #[test]
    fn drop_frame_as_seconds_matches_wall_clock() {
        let tc = |hours, minutes, seconds, frames| SmpteTime {
            hours,
            minutes,
            seconds,
            frames,
            frame_rate: MtcFrameRate::Fps2997Drop,
        };
        // 01:00:00:00 DF is one wall-clock hour to within a frame (drop-frame
        // numbering tracks realtime but is not exact: 3599.9964 s).
        assert!((tc(1, 0, 0, 0).as_seconds_f64() - 3600.0).abs() < 1.0 / 29.97);
        // At minute 1 labels 00/01 are dropped, so 00:01:00:02 is the first
        // label of the minute: frame number 1800 at 30000/1001 fps.
        assert!((tc(0, 1, 0, 2).as_seconds_f64() - 1800.0 / (30000.0 / 1001.0)).abs() < 1e-9);
        // Minute 10 is a drop exception — no labels skipped at its start.
        assert!((tc(0, 10, 0, 0).as_seconds_f64() - 17982.0 / (30000.0 / 1001.0)).abs() < 1e-9);
        // Non-drop rates are untouched by the drop-frame path.
        let ndf = SmpteTime {
            hours: 0,
            minutes: 1,
            seconds: 0,
            frames: 0,
            frame_rate: MtcFrameRate::Fps30,
        };
        assert_eq!(ndf.as_seconds_f64(), 60.0);
    }

    #[test]
    fn full_frame_rejects_short_or_wrong_subid() {
        assert!(MtcDecoder::parse_full_frame(&[0xF0, 0x7F]).is_none());
        let wrong_subid = [0xF0, 0x7F, 0x7F, 0x02, 0x01, 0x21, 0x0C, 0x22, 0x04, 0xF7];
        assert!(MtcDecoder::parse_full_frame(&wrong_subid).is_none());
    }
}
