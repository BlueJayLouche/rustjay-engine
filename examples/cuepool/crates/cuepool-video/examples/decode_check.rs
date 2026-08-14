//! Decode-check: open a video file with `VideoSource` and decode frames,
//! reporting the pixel path (GPU YUV vs swscale RGBA fallback) and decode
//! rate. Exits non-zero if the file can't be opened or yields no frames.
//!
//! Usage:
//!   cargo run -p cuepool-video --example decode_check -- <file> [num_frames]
//!
//! ponytail: verification tool, not shipped UI — frame count default (120)
//! is enough to catch codec/convert breakage without decoding whole files.

use cuepool_video::{FramePixels, VideoSource};
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("Usage: decode_check <file> [num_frames]");
    let max_frames: u32 = args.next().as_deref().unwrap_or("120").parse().unwrap();

    let mut src = match VideoSource::open(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("FAIL open {path}: {e}");
            std::process::exit(1);
        }
    };
    println!("opened {path}: {}x{}", src.width(), src.height());

    let start = Instant::now();
    let mut n = 0u32;
    let mut last_pts = 0.0;
    while n < max_frames {
        let Some(f) = src.read_frame() else { break };
        if n == 0 {
            let path_kind = match &f.pixels {
                FramePixels::Rgba(_) => "swscale -> RGBA (CPU fallback)".to_string(),
                FramePixels::YuvPlanar {
                    subsample,
                    bit_depth,
                    ..
                } => {
                    format!("GPU YUV {subsample:?} {bit_depth:?}")
                }
                FramePixels::Nv12 { .. } => "GPU NV12".to_string(),
                #[cfg(windows)]
                FramePixels::D3d11Nv12(_) => "GPU D3D11 NV12".to_string(),
            };
            println!("pixel path: {path_kind}");
        }
        assert!(f.width > 0 && f.height > 0, "degenerate frame");
        last_pts = f.pts;
        n += 1;
    }

    if n == 0 {
        eprintln!("FAIL {path}: decoded 0 frames");
        std::process::exit(1);
    }
    let dt = start.elapsed().as_secs_f64();
    println!(
        "OK {path}: {n} frames ({:.1} fps decode), last pts {last_pts:.3}s",
        n as f64 / dt
    );
}
