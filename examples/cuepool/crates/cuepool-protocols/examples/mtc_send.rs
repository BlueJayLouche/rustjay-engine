//! MTC sender — test CuePool's MTC-follow video cues without Pro Tools.
//!
//! Usage:
//!   cargo run --example mtc_send -p cuepool-protocols [port-name-substring]
//!
//! Opens a MIDI output port (first available, or the one whose name contains
//! the argument — e.g. "IAC" on macOS) and speaks 25 fps MIDI Timecode:
//!
//!   play [HH:MM:SS:FF]   stream quarter-frames from that position (default 01:00:00:00)
//!   stop                 cease quarter-frames (transport stop)
//!   locate HH:MM:SS:FF   send a full-frame SysEx locate without playing
//!   ports                list available MIDI output ports
//!   quit
//!
//! Point CuePool at the same port (it listens on ALL ports, so no setup there):
//! on macOS create an IAC Driver bus in Audio MIDI Setup and run both sides on
//! one Mac; across machines use an RTP-MIDI network session exactly as you
//! would for Pro Tools.

use midir::{MidiOutput, MidiOutputConnection};
use std::io::BufRead;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const FPS: u8 = 25;
/// 25 fps = rate bits 01 in bits 5-6 of the hour byte/nibble.
const RATE_BITS: u8 = 0x01;

enum Cmd {
    Play(u8, u8, u8, u8),   // h, m, s, f
    Locate(u8, u8, u8, u8), // full-frame SysEx, no stream
    Stop,
    Quit,
}

fn main() {
    let filter = std::env::args().nth(1);
    let out = match MidiOutput::new("CuePool MTC Sender") {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Could not open MIDI: {e}");
            std::process::exit(1);
        }
    };
    let ports = out.ports();
    if ports.is_empty() {
        eprintln!("No MIDI output ports available.");
        eprintln!("On macOS, enable an IAC Driver bus (Audio MIDI Setup → MIDI Studio).");
        std::process::exit(1);
    }
    let port = match &filter {
        Some(f) => ports
            .iter()
            .find(|p| {
                out.port_name(p)
                    .map(|n| n.contains(f.as_str()))
                    .unwrap_or(false)
            })
            .unwrap_or_else(|| {
                eprintln!("No port matching '{f}'. Available:");
                for p in &ports {
                    eprintln!("  {}", out.port_name(p).unwrap_or_default());
                }
                std::process::exit(1);
            }),
        None => &ports[0],
    };
    let port_name = out.port_name(port).unwrap_or_default();
    let conn = match out.connect(port, "mtc-send") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to open {port_name}: {e}");
            std::process::exit(1);
        }
    };
    println!("Sending MTC (25 fps) on: {port_name}");

    let (tx, rx) = mpsc::channel::<Cmd>();
    std::thread::spawn(move || sender_loop(conn, rx));

    println!("Commands: play [HH:MM:SS:FF] | stop | locate HH:MM:SS:FF | quit");
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let mut parts = line.split_whitespace();
        let Some(cmd) = parts.next() else { continue };
        match cmd {
            "play" => {
                let (h, m, s, f) = parts
                    .next()
                    .and_then(parse_timecode)
                    .unwrap_or((1, 0, 0, 0));
                println!("Playing from {h:02}:{m:02}:{s:02}:{f:02}");
                let _ = tx.send(Cmd::Play(h, m, s, f));
            }
            "stop" => {
                println!("Stopped.");
                let _ = tx.send(Cmd::Stop);
            }
            "locate" => match parts.next().and_then(parse_timecode) {
                Some((h, m, s, f)) => {
                    println!("Located at {h:02}:{m:02}:{s:02}:{f:02} (not playing)");
                    let _ = tx.send(Cmd::Locate(h, m, s, f));
                }
                None => println!("Usage: locate HH:MM:SS:FF"),
            },
            "quit" | "exit" => {
                let _ = tx.send(Cmd::Quit);
                break;
            }
            "help" => println!("Commands: play [HH:MM:SS:FF] | stop | locate HH:MM:SS:FF | quit"),
            other => println!("Unknown command '{other}'. Type 'help'."),
        }
    }
    let _ = tx.send(Cmd::Quit);
}

/// Parse `HH:MM:SS:FF` (frames at 25 fps).
fn parse_timecode(text: &str) -> Option<(u8, u8, u8, u8)> {
    let mut it = text.split(':');
    let h = it.next()?.parse().ok()?;
    let m = it.next()?.parse().ok()?;
    let s = it.next()?.parse().ok()?;
    let f = it.next()?.parse().ok()?;
    Some((h, m, s, f))
}

fn sender_loop(mut conn: MidiOutputConnection, rx: mpsc::Receiver<Cmd>) {
    let mut playing = false;
    let mut pos: (u8, u8, u8, u8) = (1, 0, 0, 0);
    // Quarter-frames: 8 per frame at 25 fps = one every 10 ms, nibble 0..7.
    let mut next_qf = Instant::now();
    let mut nibble: u8 = 0;
    loop {
        // Drain pending commands without blocking the stream.
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Play(h, m, s, f) => {
                    pos = (h, m, s, f);
                    playing = true;
                    nibble = 0;
                    next_qf = Instant::now();
                }
                Cmd::Locate(h, m, s, f) => {
                    // Full-frame: F0 7F 7F 01 01 hr mn sc fr F7 (25fps rate
                    // bits 01 in hr bits 5-6). A locate, not a play.
                    let hr = (RATE_BITS << 5) | (h & 0x1F);
                    let _ = conn.send(&[0xF0, 0x7F, 0x7F, 0x01, 0x01, hr, m, s, f, 0xF7]);
                }
                Cmd::Stop => playing = false,
                Cmd::Quit => return,
            }
        }
        if playing && Instant::now() >= next_qf {
            next_qf += Duration::from_millis(10);
            let _ = conn.send(&[0xF1, quarter_frame(nibble, pos)]);
            nibble += 1;
            if nibble == 8 {
                nibble = 0;
                pos = advance_frame(pos);
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Data byte for quarter-frame `n` of the standard 0..7 sequence.
fn quarter_frame(n: u8, (h, m, s, f): (u8, u8, u8, u8)) -> u8 {
    let value = match n {
        0 => f & 0x0F,
        1 => (f >> 4) & 0x01,
        2 => s & 0x0F,
        3 => (s >> 4) & 0x07,
        4 => m & 0x0F,
        5 => (m >> 4) & 0x07,
        6 => h & 0x0F,
        _ => ((RATE_BITS << 1) | ((h >> 4) & 0x01)) & 0x07,
    };
    (n << 4) | value
}

/// Advance one frame at 25 fps.
fn advance_frame((h, m, s, f): (u8, u8, u8, u8)) -> (u8, u8, u8, u8) {
    let f = f + 1;
    if f < FPS {
        return (h, m, s, f);
    }
    let s = s + 1;
    if s < 60 {
        return (h, m, s, 0);
    }
    let m = m + 1;
    if m < 60 {
        return (h, m, 0, 0);
    }
    ((h + 1) % 24, 0, 0, 0)
}
