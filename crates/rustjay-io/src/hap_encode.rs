//! Converting any video into a HAP clip: ffmpeg decodes, `hap-qt` encodes.
//!
//! HAP is what a VJ app wants on the playback side — the GPU decodes it — but
//! nothing produces it by accident, so an app that plays HAP needs a way to
//! make it. Lifted out of VP-404, which had this to itself.

/// Codec for opaque clips.
///
/// Hap Q is the same 16 bytes per block as Hap5 but carries far better colour,
/// so it is free quality for any clip without transparency. Its YCoCg decode is
/// already handled in `hap_convert.wgsl`.
///
/// ponytail: switch to `HapFormat::Hap1` when pad-count bandwidth matters more
/// than colour — 8 bytes per block halves VRAM and per-frame upload traffic
/// (1080p: 1.04 MB/frame vs 2.07), which is the binding constraint on the Pi
/// target with many pads live.
const OPAQUE_FORMAT: hap_qt::HapFormat = hap_qt::HapFormat::HapY;

/// Whether an ffmpeg pixel format carries an alpha channel.
///
/// ffmpeg names these consistently, so matching on the name beats maintaining a
/// full table. Note `gray`/`ya8` — a bare `contains("a")` would be wrong.
pub fn pix_fmt_has_alpha(pix_fmt: &str) -> bool {
    const ALPHA: [&str; 8] = ["yuva", "rgba", "bgra", "argb", "abgr", "gbrap", "ya8", "ya16"];
    ALPHA.iter().any(|a| pix_fmt.contains(a))
}

/// Convert any video to HAP using ffmpeg for *decode* and hap-qt for *encode*.
///
/// Both `ffmpeg` and `ffprobe` must be on PATH; a missing one is the error.
///
/// ffmpeg decodes to raw RGBA via stdout; we feed that into `HapFrameEncoder`
/// plus `QtHapWriter`. The codec comes from the source's pixel format, via
/// the [`pix_fmt_has_alpha`] check and the [`OPAQUE_FORMAT`] constant.
///
/// This avoids the broken `-c:v hap` ffmpeg path (the homebrew ffmpeg 8 build
/// has no HAP encoder) and the invalid `-chunks` flag removed in ffmpeg 6+.
pub fn ffmpeg_to_hap(src: &std::path::Path, dst: &std::path::Path) -> anyhow::Result<()> {
    use std::io::{BufReader, Read};
    use std::process::{Command, Stdio};
    use hap_qt::{CompressionMode, DxtQuality, HapFormat, HapFrameEncoder, QtHapWriter, VideoConfig};
    use rayon::prelude::*;

    // --- probe: width, height, fps, pixel format via ffprobe ---------------
    // key=value output, not positional CSV: ffprobe emits fields in its own
    // order, not the order they were requested, so positions are not stable.
    let probe = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height,r_frame_rate,pix_fmt",
            "-of", "default=noprint_wrappers=1",
            src.to_str().unwrap_or_default(),
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("ffprobe not found: {e}"))?;
    if !probe.status.success() {
        anyhow::bail!("ffprobe failed: {}", String::from_utf8_lossy(&probe.stderr));
    }
    let probe_str = String::from_utf8_lossy(&probe.stdout);
    let field = |key: &str| -> Option<&str> {
        probe_str
            .lines()
            .find_map(|l| l.trim().strip_prefix(key)?.strip_prefix('='))
    };
    let width: u32 = field("width").and_then(|s| s.parse().ok()).unwrap_or(1920);
    let height: u32 = field("height").and_then(|s| s.parse().ok()).unwrap_or(1080);
    let fps: f32 = field("r_frame_rate").map(|s| {
        if let Some((n, d)) = s.split_once('/') {
            let n: f32 = n.parse().unwrap_or(30.0);
            let d: f32 = d.parse().unwrap_or(1.0);
            if d > 0.0 { n / d } else { 30.0 }
        } else {
            s.parse().unwrap_or(30.0)
        }
    }).unwrap_or(30.0);
    let pix_fmt = field("pix_fmt").unwrap_or("");

    log::info!(
        "hap-encode: {src:?} → {dst:?} ({width}x{height} @ {fps:.2} fps)"
    );

    // --- decode: ffmpeg → raw RGBA on stdout ------------------------------
    let mut child = Command::new("ffmpeg")
        .args([
            "-y", "-i", src.to_str().unwrap_or_default(),
            "-f", "rawvideo", "-pix_fmt", "rgba", "-an", "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("ffmpeg not found: {e}"))?;

    let stdout = child.stdout.take().unwrap();
    // Drain stderr in background to avoid pipe deadlock.
    let stderr = child.stderr.take();
    let stderr_thread = std::thread::spawn(move || {
        let Some(s) = stderr else { return String::new() };
        let mut buf = String::new();
        let _ = BufReader::new(s).read_to_string(&mut buf);
        buf
    });

    // --- encode: hap-qt (Snappy) ------------------------------------------
    // Only pay for an alpha channel when the source actually has one. Hap5 and
    // Hap Q are both 16 bytes per block, so encoding an opaque clip as Hap5
    // costs the same as Hap Q and simply throws the colour quality away.
    let hap_format = if pix_fmt_has_alpha(pix_fmt) {
        HapFormat::Hap5
    } else {
        OPAQUE_FORMAT
    };
    log::info!("hap-encode: pix_fmt {pix_fmt:?} -> {hap_format:?}");
    let video_config = VideoConfig::new(width, height, fps, hap_format);
    let mut writer = QtHapWriter::create(dst, video_config)
        .map_err(|e| anyhow::anyhow!("QtHapWriter::create failed: {e}"))?;

    let frame_size = (width * height * 4) as usize;
    let mut reader = BufReader::with_capacity(frame_size * 2, stdout);
    let mut frame_count = 0u32;

    const BATCH: usize = 16;
    let mut batch: Vec<Vec<u8>> = Vec::with_capacity(BATCH);

    loop {
        batch.clear();
        for _ in 0..BATCH {
            let mut buf = vec![0u8; frame_size];
            match reader.read_exact(&mut buf) {
                Ok(()) => batch.push(buf),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => anyhow::bail!("ffmpeg stdout read error: {e}"),
            }
        }
        if batch.is_empty() { break; }

        // Parallel DXT5 + Snappy compress
        let encoded: Vec<Vec<u8>> = batch
            .par_iter()
            .map(|frame| {
                let mut enc = HapFrameEncoder::new(hap_format, width, height)
                    .map_err(|e| anyhow::anyhow!("HapFrameEncoder::new: {e}"))?;
                enc.set_compression(CompressionMode::Snappy);
                // Explicit rather than relying on the default. Measured on
                // BC1, texpresso's Best scores slightly *worse* than Balanced
                // (17.15 vs 17.26 dB) for twice the time, so Balanced is the
                // top of the useful CPU range, not a compromise.
                enc.set_quality(DxtQuality::Balanced);
                enc.encode(frame).map_err(|e| anyhow::anyhow!("encode frame: {e}"))
            })
            .collect::<anyhow::Result<_>>()?;

        for hap_frame in &encoded {
            writer.write_frame(hap_frame)
                .map_err(|e| anyhow::anyhow!("write_frame: {e}"))?;
            frame_count += 1;
        }
    }

    writer.finalize().map_err(|e| anyhow::anyhow!("finalize: {e}"))?;
    let _ = child.wait();

    if let Ok(err_str) = stderr_thread.join()
        && !err_str.is_empty()
    {
        log::debug!("ffmpeg stderr (last line): {}", err_str.lines().last().unwrap_or(""));
    }

    log::info!("hap-encode done: {frame_count} frames → {dst:?}");
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::pix_fmt_has_alpha;

    #[test]
    fn alpha_detection_matches_ffmpeg_pixel_formats() {
        for fmt in ["yuva420p", "yuva444p10le", "rgba", "bgra", "argb", "abgr", "gbrap", "ya8", "ya16le", "rgba64be"] {
            assert!(pix_fmt_has_alpha(fmt), "{fmt} should be detected as having alpha");
        }
        // "gray" and "gbrp" are the traps a naive substring check would fail.
        for fmt in ["yuv420p", "yuv444p", "rgb24", "bgr0", "gray", "gbrp", "nv12", "yuv422p10le"] {
            assert!(!pix_fmt_has_alpha(fmt), "{fmt} should be detected as opaque");
        }
    }
}
