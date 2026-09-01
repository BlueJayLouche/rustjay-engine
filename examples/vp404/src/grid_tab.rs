//! The "Pads" egui tab: a 4×4 momentary pad grid + a selected-pad panel.
//!
//! Buttons are momentary: press sets `pad<i>_trig` param to 1.0, release sets it
//! to 0.0. The edge detector in `prepare()` fires `trigger()`/`release()` on the
//! pad, so MIDI/OSC/web and the grid button all share the same code path.
//! Load/Clear/SetMode/SetRange are still posted as [`PadCmd`]s (they need render-
//! thread resources). Pad state is read back from the published roster.
//!
//! Per-pad mix/playback params are driven through the engine (so MIDI/OSC/LFO
//! stay authoritative) using `param_slider` / `param_slider_int` with the channel
//! UUID prefix (`ch_pad<N>_opacity` / `ch_pad<N>_speed` / `ch_pad<N>_mode` /
//! `ch_pad<N>_division`).

use std::path::PathBuf;
use std::thread::JoinHandle;

use rustjay_core::lfo::BEAT_DIVISION_NAMES;
use rustjay_engine::prelude::*;

#[cfg(feature = "capture")]
use crate::bank::SamplerStatus;
use crate::bank::{BankHandle, PadCmd, PAD_COUNT};
use crate::pad::{PlaybackMode, TriggerMode};

const COLS: usize = 4;

/// An in-flight ffmpeg HAP conversion for a non-HAP source file.
struct ConvertJob {
    target_pad: usize,
    output: PathBuf,
    handle: JoinHandle<anyhow::Result<()>>,
}

pub struct PadGridTab {
    handle: BankHandle,
    selected: usize,
    /// Per-pad "button currently held" latch, for edge-detecting press/release.
    was_down: Vec<bool>,
    /// Background ffmpeg conversions; drained each frame.
    convert_jobs: Vec<ConvertJob>,
    /// Status message shown below the Load button.
    load_status: String,
    /// Frame count for the next Record (capture feature).
    #[cfg(feature = "capture")]
    record_frames: u32,
}

impl PadGridTab {
    pub fn new(handle: BankHandle) -> Self {
        Self {
            handle,
            selected: 0,
            was_down: vec![false; PAD_COUNT],
            convert_jobs: Vec::new(),
            load_status: String::new(),
            #[cfg(feature = "capture")]
            record_frames: 120,
        }
    }

    fn poll_convert_jobs(&mut self) {
        let mut i = 0;
        while i < self.convert_jobs.len() {
            if self.convert_jobs[i].handle.is_finished() {
                let job = self.convert_jobs.remove(i);
                match job.handle.join() {
                    Ok(Ok(())) => {
                        self.load_status = String::new();
                        self.handle.post(PadCmd::Load(job.target_pad, job.output));
                    }
                    Ok(Err(e)) => {
                        self.load_status = format!("Convert failed: {e}");
                    }
                    Err(_) => {
                        self.load_status = "Convert thread panicked".to_string();
                    }
                }
            } else {
                i += 1;
            }
        }
    }

    fn load_or_convert(&mut self, path: PathBuf, target_pad: usize) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
        if name.ends_with(".hap.mov") {
            self.handle.post(PadCmd::Load(target_pad, path));
            return;
        }
        // Check for an already-converted sidecar.
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
        let stem = stem.trim_end_matches(".hap");
        let sidecar = path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join(format!("{stem}_converted.hap.mov"));
        if sidecar.exists() {
            self.handle.post(PadCmd::Load(target_pad, sidecar));
            return;
        }
        // Kick off ffmpeg conversion.
        let output = sidecar;
        let src = path;
        let out = output.clone();
        let handle = std::thread::spawn(move || ffmpeg_to_hap(&src, &out));
        self.load_status = "Converting…".to_string();
        self.convert_jobs.push(ConvertJob { target_pad, output, handle });
    }
}

impl AnyEguiTab for PadGridTab {
    fn name(&self) -> &str {
        "Pads"
    }

    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        _app_state: &mut dyn std::any::Any,
        engine: &mut EngineState,
    ) {
        self.poll_convert_jobs();

        let roster = self.handle.roster();
        let n = roster.len().max(PAD_COUNT);
        if self.was_down.len() < n {
            self.was_down.resize(n, false);
        }

        ui.heading("Pads");
        let rows = n.div_ceil(COLS);
        for row in 0..rows {
            ui.horizontal(|ui| {
                for col in 0..COLS {
                    let i = row * COLS + col;
                    if i >= n {
                        break;
                    }
                    let info = roster.get(i).cloned().unwrap_or_default();
                    let [r, g, b] = info.color;
                    let base = if info.loaded {
                        egui::Color32::from_rgb(r, g, b)
                    } else {
                        egui::Color32::from_gray(60)
                    };
                    let fill = if info.playing {
                        base
                    } else {
                        base.gamma_multiply(0.45)
                    };
                    let label = if info.playing {
                        format!("▶ {}", i + 1)
                    } else {
                        format!("{}", i + 1)
                    };
                    let mut btn = egui::Button::new(label)
                        .fill(fill)
                        .min_size(egui::vec2(56.0, 56.0));
                    if self.selected == i {
                        btn = btn.stroke(egui::Stroke::new(2.0_f32, egui::Color32::WHITE));
                    }
                    let resp = ui.add(btn);
                    if resp.clicked() || resp.is_pointer_button_down_on() {
                        self.selected = i;
                    }
                    let down = resp.is_pointer_button_down_on();
                    if down && !self.was_down[i] {
                        engine.set_param_base(&format!("pad{i}_trig"), 1.0);
                    } else if !down && self.was_down[i] {
                        engine.set_param_base(&format!("pad{i}_trig"), 0.0);
                    }
                    self.was_down[i] = down;
                }
            });
        }

        ui.separator();

        // --- Selected pad panel ------------------------------------------
        let sel = self.selected;
        let info = roster.get(sel).cloned().unwrap_or_default();

        // Show clip name only when it's distinct from the pad number.
        let pad_label = format!("Pad {}", sel + 1);
        let heading = if info.loaded && !info.name.is_empty() && info.name != pad_label {
            format!("{pad_label} — {}", info.name)
        } else {
            pad_label
        };
        ui.heading(heading);

        ui.horizontal(|ui| {
            let is_converting = !self.convert_jobs.is_empty();
            if ui
                .add_enabled(!is_converting, egui::Button::new("Load…"))
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Video", &["mov", "mp4", "m4v", "avi", "mkv"])
                    .pick_file()
                {
                    self.load_or_convert(path, sel);
                }
            }
            if ui.button("Clear").clicked() {
                self.handle.post(PadCmd::Clear(sel));
            }
            if is_converting {
                ui.spinner();
            } else if !self.load_status.is_empty() {
                ui.label(self.load_status.clone());
            }

            #[cfg(feature = "capture")]
            {
                let status = self.handle.sampler_status();
                let idle = matches!(status, SamplerStatus::Idle | SamplerStatus::Error);
                let label = match status {
                    SamplerStatus::Idle | SamplerStatus::Error => "⏺ Record",
                    SamplerStatus::Recording => "Recording…",
                    SamplerStatus::Encoding => "Encoding…",
                };
                if ui.add_enabled(idle, egui::Button::new(label)).clicked() {
                    self.handle.post(PadCmd::StartSampling(sel, self.record_frames));
                }
                ui.add_enabled(
                    idle,
                    egui::DragValue::new(&mut self.record_frames)
                        .range(1..=9000)
                        .suffix(" fr"),
                );
                if !idle && ui.button("Cancel").clicked() {
                    self.handle.post(PadCmd::StopSampling);
                }
            }
        });

        let mut mode = info.trigger_mode;
        egui::ComboBox::from_label("Trigger mode")
            .selected_text(format!("{mode:?}"))
            .show_ui(ui, |ui| {
                for m in [TriggerMode::Gate, TriggerMode::Latch, TriggerMode::OneShot] {
                    if ui
                        .selectable_value(&mut mode, m, format!("{m:?}"))
                        .clicked()
                    {
                        self.handle.post(PadCmd::SetMode(sel, m));
                    }
                }
            });

        let mut looped = info.loop_enabled;
        if ui.checkbox(&mut looped, "Loop").clicked() {
            self.handle.post(PadCmd::SetLoop(sel, looped));
        }

        let opacity_id = format!("ch_pad{sel}_opacity");
        param_slider(ui, engine, &opacity_id, "Opacity", 0.0, 1.0);

        {
            use rustjay_mixer::BlendMode;
            let blend_id = format!("ch_pad{sel}_blend");
            let mut blend_idx = engine.get_param_base(&blend_id).unwrap_or(0.0).round() as u32;
            let current = BlendMode::from_index(blend_idx).unwrap_or_default();
            egui::ComboBox::from_label("Blend")
                .selected_text(current.short_name())
                .show_ui(ui, |ui| {
                    for mode in BlendMode::all() {
                        if ui.selectable_value(&mut blend_idx, mode.to_index(), mode.short_name()).clicked() {
                            engine.set_param_base(&blend_id, blend_idx as f32);
                        }
                    }
                });
        }

        let mode_id = format!("ch_pad{sel}_mode");
        let mut mode_val = engine.get_param_base(&mode_id).unwrap_or(0.0).round() as usize;
        let mode_label =
            PlaybackMode::labels()[mode_val.clamp(0, PlaybackMode::labels().len() - 1)];
        egui::ComboBox::from_label("Playback mode")
            .selected_text(mode_label)
            .show_ui(ui, |ui| {
                for (i, label) in PlaybackMode::labels().iter().enumerate() {
                    if ui.selectable_value(&mut mode_val, i, *label).clicked() {
                        engine.set_param_base(&mode_id, i as f32);
                    }
                }
            });

        if PlaybackMode::from_index(mode_val) == PlaybackMode::Free {
            let speed_id = format!("ch_pad{sel}_speed");
            param_slider(ui, engine, &speed_id, "Speed", -5.0, 5.0);
        } else {
            let div_label =
                BEAT_DIVISION_NAMES[info.beat_division.clamp(0, BEAT_DIVISION_NAMES.len() - 1)];
            ui.label(format!("Speed: locked to beat ({div_label})"));
            // Synced ignores the speed magnitude but uses its sign for direction,
            // so the hidden slider becomes a checkbox here.
            let speed_id = format!("ch_pad{sel}_speed");
            let speed = engine.get_param_base(&speed_id).unwrap_or(1.0);
            let mut reverse = speed < 0.0;
            if ui.checkbox(&mut reverse, "Reverse").clicked() {
                let mag = if speed.abs() > 0.0 { speed.abs() } else { 1.0 };
                engine.set_param_base(&speed_id, if reverse { -mag } else { mag });
            }
        }

        let division_id = format!("ch_pad{sel}_division");
        let mut div_val = engine.get_param_base(&division_id).unwrap_or(2.0).round() as usize;
        egui::ComboBox::from_label("Beat division")
            .selected_text(BEAT_DIVISION_NAMES[div_val.clamp(0, BEAT_DIVISION_NAMES.len() - 1)])
            .show_ui(ui, |ui| {
                for (i, label) in BEAT_DIVISION_NAMES.iter().enumerate() {
                    if ui.selectable_value(&mut div_val, i, *label).clicked() {
                        engine.set_param_base(&division_id, i as f32);
                    }
                }
            });

        if info.loaded && info.frame_count > 0 {
            ui.separator();
            ui.label("Range");
            let mut in_pt = info.in_point as i32;
            let mut out_pt = info.out_point as i32;
            let max = info.frame_count.saturating_sub(1) as i32;
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut in_pt).speed(1.0).range(0..=max));
                ui.label("in");
                ui.add(egui::DragValue::new(&mut out_pt).speed(1.0).range(0..=max));
                ui.label("out");
            });
            if in_pt != info.in_point as i32 || out_pt != info.out_point as i32 {
                self.handle
                    .post(PadCmd::SetRange(sel, in_pt as u32, out_pt as u32));
            }
        }

        if info.loaded {
            ui.add(egui::ProgressBar::new(info.progress).desired_width(220.0));
        }

        // --- Chroma/Luma key controls ----------------------------------------
        ui.separator();
        let key_mode_id = format!("ch_pad{sel}_key_mode");
        let mut key_mode = engine.get_param_base(&key_mode_id).unwrap_or(0.0).round() as usize;
        egui::ComboBox::from_label("Key mode")
            .selected_text(["None", "Chroma", "Luma"][key_mode.clamp(0, 2)])
            .show_ui(ui, |ui| {
                for (i, label) in ["None", "Chroma", "Luma"].iter().enumerate() {
                    if ui.selectable_value(&mut key_mode, i, *label).clicked() {
                        engine.set_param_base(&key_mode_id, i as f32);
                    }
                }
            });

        if key_mode == 1 {
            let prefix = format!("ch_pad{sel}_");
            key_color_picker(ui, engine, &prefix);
            param_slider(ui, engine, &format!("ch_pad{sel}_key_threshold"), "Threshold", 0.0, 1.0);
            param_slider(ui, engine, &format!("ch_pad{sel}_key_smoothness"), "Smoothness", 0.0, 1.0);
        } else if key_mode == 2 {
            param_slider(ui, engine, &format!("ch_pad{sel}_key_threshold"), "Threshold", 0.0, 1.0);
            param_slider(ui, engine, &format!("ch_pad{sel}_key_smoothness"), "Smoothness", 0.0, 1.0);
            let invert_id = format!("ch_pad{sel}_key_luma_invert");
            let mut invert = engine.get_param_base(&invert_id).unwrap_or(0.0) > 0.5;
            if ui.checkbox(&mut invert, "Invert").changed() {
                engine.set_param_base(&invert_id, if invert { 1.0 } else { 0.0 });
            }
        }
    }
}

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
fn pix_fmt_has_alpha(pix_fmt: &str) -> bool {
    const ALPHA: [&str; 8] = ["yuva", "rgba", "bgra", "argb", "abgr", "gbrap", "ya8", "ya16"];
    ALPHA.iter().any(|a| pix_fmt.contains(a))
}

/// Convert any video to HAP using ffmpeg for *decode* and hap-qt for *encode*.
///
/// ffmpeg decodes to raw RGBA via stdout; we feed that into `HapFrameEncoder`
/// plus `QtHapWriter`. The codec comes from the source's pixel format, via
/// the [`pix_fmt_has_alpha`] check and the [`OPAQUE_FORMAT`] constant.
///
/// This avoids the broken `-c:v hap` ffmpeg path (the homebrew ffmpeg 8 build
/// has no HAP encoder) and the invalid `-chunks` flag removed in ffmpeg 6+.
fn ffmpeg_to_hap(src: &std::path::Path, dst: &std::path::Path) -> anyhow::Result<()> {
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
        "VP-404 convert: {src:?} → {dst:?} ({width}x{height} @ {fps:.2} fps)"
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
    log::info!("VP-404 convert: pix_fmt {pix_fmt:?} -> {hap_format:?}");
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

    if let Ok(err_str) = stderr_thread.join() {
        if !err_str.is_empty() {
            log::debug!("ffmpeg stderr (last line): {}", err_str.lines().last().unwrap_or(""));
        }
    }

    log::info!("VP-404 convert done: {frame_count} frames → {dst:?}");
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
