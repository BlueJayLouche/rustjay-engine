//! Drive a recovered `ledmap.json` live over sACN from an animated test frame.
//!
//! Proves the playback half end-to-end on real hardware without touching the
//! engine: load a map, sample a moving color field at each LED's `(u,v)`, push
//! the result over sACN at ~30fps. A spatially-moving field means a correctly
//! mapped strip shows motion that tracks physical position.
//!
//! Usage:
//! ```text
//! cargo run -p rustjay-ledmap --example playback -- ledmap.json
//! ```
//!
//! In the engine proper this same `PointMap::sample` call is fed the real output
//! texture read back to BGRA (see DESIGN.md, Milestone 2 wiring).

use std::time::{Duration, Instant};

use rustjay_ledmap::PointMap;
use rustjay_lighting::{Dest, DmxSender, SacnTransport};

const W: usize = 320;
const H: usize = 180;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "ledmap.json".into());
    let pm = PointMap::load(&path)?;
    println!("Loaded {} LEDs from {path}", pm.len());
    if pm.is_empty() {
        anyhow::bail!("map has no drivable LEDs");
    }

    let transport = SacnTransport::new(Dest::Multicast, 100, "rustjay-ledmap-playback")?;
    let sender = DmxSender::spawn(Box::new(transport), 44.0);
    println!("Streaming sACN — Ctrl-C to stop.");

    // ponytail: Ctrl-C terminates without a blackout; the controller holds the
    // last frame. Add a ctrlc handler + sender.submit(blackout) if that matters.
    let start = Instant::now();
    let mut bgra = vec![0u8; W * H * 4];
    loop {
        let t = start.elapsed().as_secs_f32();
        render_field(&mut bgra, t);
        sender.submit(pm.sample(&bgra, W, H));
        std::thread::sleep(Duration::from_millis(33));
    }
}

/// Fill `bgra` with a moving color field — independent sinusoids per channel so
/// motion is visible along any strip orientation.
fn render_field(bgra: &mut [u8], t: f32) {
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) * 4;
            let r = wave(x as f32 * 0.05 + t);
            let g = wave(y as f32 * 0.05 + t * 1.3);
            let b = wave((x + y) as f32 * 0.05 + t * 0.7);
            bgra[i] = b;
            bgra[i + 1] = g;
            bgra[i + 2] = r;
            bgra[i + 3] = 255;
        }
    }
}

#[inline]
fn wave(p: f32) -> u8 {
    (p.sin() * 127.0 + 128.0) as u8
}
