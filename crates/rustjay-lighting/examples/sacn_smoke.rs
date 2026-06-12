//! Proof-of-life: stream a moving rainbow across the first 6 RGB fixtures
//! (18 channels) of universe 1.
//!
//! Usage:
//!   cargo run -p rustjay-lighting --example sacn_smoke            # sACN multicast
//!   cargo run -p rustjay-lighting --example sacn_smoke -- artnet  # Art-Net broadcast
//!
//! Verify with sACNView / an Art-Net monitor (QLC+, Resolume, etc.). Ctrl-C to stop.

use std::time::{Duration, Instant};

use rustjay_lighting::{ArtNetTransport, Dest, DmxFrame, DmxSender, DmxTransport, SacnTransport};

const FIXTURES: usize = 6;

fn main() {
    let proto = std::env::args().nth(1).unwrap_or_else(|| "sacn".into());

    let transport: Box<dyn DmxTransport> = match proto.as_str() {
        "artnet" => {
            println!("Art-Net (broadcast) → universe 1");
            Box::new(ArtNetTransport::new(Dest::Broadcast).expect("artnet socket"))
        }
        _ => {
            println!("sACN (multicast) → universe 1");
            Box::new(SacnTransport::new(Dest::Multicast, 100, "vjarda").expect("sacn socket"))
        }
    };

    let sender = DmxSender::spawn(transport, 30.0);
    let start = Instant::now();

    println!("streaming a moving rainbow — Ctrl-C to stop");
    loop {
        let t = start.elapsed().as_secs_f32();
        let mut frame = DmxFrame::new();
        let u = frame.universe_mut(1);
        for i in 0..FIXTURES {
            let hue = (t * 0.15 + i as f32 / FIXTURES as f32).fract();
            let [r, g, b] = hsv_to_rgb(hue);
            let base = i * 3;
            u[base] = r;
            u[base + 1] = g;
            u[base + 2] = b;
        }
        sender.submit(frame);
        std::thread::sleep(Duration::from_millis(33));
    }
}

/// Minimal HSV→RGB with S=V=1, hue in 0..1.
fn hsv_to_rgb(h: f32) -> [u8; 3] {
    let h6 = h * 6.0;
    let c = 1.0;
    let x = c * (1.0 - ((h6 % 2.0) - 1.0).abs());
    let (r, g, b) = match h6 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}
