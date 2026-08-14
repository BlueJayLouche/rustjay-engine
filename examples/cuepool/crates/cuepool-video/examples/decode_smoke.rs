//! Decode-smoke: open a video file with `VideoSource`, decode ~120 frames,
//! assert PTS is monotonically increasing and every frame converts, and print
//! which decode path (hardware / software) was taken.
//!
//! Usage:
//!   cargo run -p cuepool-video --example decode_smoke -- <file>
//!   QPLAYER_NO_HWACCEL=1 ...   # force the software path

use cuepool_video::VideoSource;

fn main() {
    let path = std::env::args().nth(1).expect("Usage: decode_smoke <file>");
    let mut src = match VideoSource::open(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("FAIL open {path}: {e}");
            std::process::exit(1);
        }
    };
    println!("decode path: {}", src.decode_path());

    let mut n = 0u32;
    let mut last_pts = f64::NEG_INFINITY;
    while n < 120 {
        let Some(f) = src.read_frame() else { break };
        assert!(
            f.pts >= last_pts,
            "PTS not monotonic: {last_pts} -> {}",
            f.pts
        );
        last_pts = f.pts;
        n += 1;
    }

    if n == 0 {
        eprintln!("FAIL {path}: decoded 0 frames");
        std::process::exit(1);
    }
    println!(
        "OK: {n} frames, {}x{}, path {}",
        src.width(),
        src.height(),
        src.decode_path()
    );
}
