//! Waveform display — generates and renders audio peak data.
//!
//! Peak files (`.qpek`) cache decoded waveform data for instant reload.

use egui::Color32;
use qplayer_audio::SampleProvider;
use std::io::{Read, Write};

const QPEK_MAGIC: &[u8] = b"QPEK";
const QPEK_VERSION: u32 = 1;

/// Generate or load cached peak data for a waveform.
/// Returns `num_bars` pairs of (min, max) sample values in [-1, 1].
pub fn generate_peaks(path: &str, num_bars: usize) -> Option<Vec<(f32, f32)>> {
    // Try loading from cached peak file first
    if let Some(peaks) = load_peaks(path) {
        return Some(peaks);
    }

    let decoder = qplayer_audio::FileDecoder::open(path).ok()?;
    let length = decoder.length()?;
    if length == 0 || num_bars == 0 {
        return None;
    }

    let chunk_size = (length / num_bars).max(1);
    let mut buffer = vec![0.0f32; chunk_size];
    let mut peaks = Vec::with_capacity(num_bars);

    for _ in 0..num_bars {
        let read = decoder.read(&mut buffer);
        if read == 0 {
            break;
        }
        let mut min_val = 0.0f32;
        let mut max_val = 0.0f32;
        for sample in &buffer[..read] {
            min_val = min_val.min(*sample);
            max_val = max_val.max(*sample);
        }
        peaks.push((min_val, max_val));
    }

    // Save to cache for next time
    let _ = save_peaks(path, &peaks);

    Some(peaks)
}

/// Load peaks from a `.qpek` sidecar file if it exists and is valid.
fn load_peaks(audio_path: &str) -> Option<Vec<(f32, f32)>> {
    let peak_path = format!("{}.qpek", audio_path);
    let mut file = std::fs::File::open(&peak_path).ok()?;

    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).ok()?;
    if &magic != QPEK_MAGIC {
        return None;
    }

    let mut version_buf = [0u8; 4];
    file.read_exact(&mut version_buf).ok()?;
    let version = u32::from_le_bytes(version_buf);
    if version != QPEK_VERSION {
        return None;
    }

    let mut count_buf = [0u8; 4];
    file.read_exact(&mut count_buf).ok()?;
    let count = u32::from_le_bytes(count_buf) as usize;

    let mut peaks = Vec::with_capacity(count);
    for _ in 0..count {
        let mut min_buf = [0u8; 4];
        let mut max_buf = [0u8; 4];
        file.read_exact(&mut min_buf).ok()?;
        file.read_exact(&mut max_buf).ok()?;
        peaks.push((
            f32::from_le_bytes(min_buf),
            f32::from_le_bytes(max_buf),
        ));
    }

    Some(peaks)
}

/// Save peaks to a `.qpek` sidecar file.
fn save_peaks(audio_path: &str, peaks: &[(f32, f32)]) -> std::io::Result<()> {
    let peak_path = format!("{}.qpek", audio_path);
    let mut file = std::fs::File::create(&peak_path)?;

    file.write_all(QPEK_MAGIC)?;
    file.write_all(&QPEK_VERSION.to_le_bytes())?;
    file.write_all(&(peaks.len() as u32).to_le_bytes())?;

    for (min_val, max_val) in peaks {
        file.write_all(&min_val.to_le_bytes())?;
        file.write_all(&max_val.to_le_bytes())?;
    }

    Ok(())
}

/// Draw a waveform from pre-computed peak data with zoom and pan support.
///
/// `zoom` > 1.0 zooms in horizontally. `scroll_offset` is in bars from the left.
/// `height` is the desired height in points (default 48.0 for the inspector mini-view).
/// Returns the updated (zoom, scroll_offset) after handling user input.
pub fn draw(ui: &mut egui::Ui, peaks: &[(f32, f32)], zoom: f32, scroll_offset: f32, height: f32) -> (f32, f32) {
    let desired_size = egui::vec2(ui.available_width(), height);
    let _id = ui.auto_id_with("waveform");
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
    let painter = ui.painter();

    // Background
    painter.rect_filled(rect, 2.0, Color32::from_rgb(30, 30, 30));

    if peaks.is_empty() {
        return (zoom, scroll_offset);
    }

    // Handle zoom (mouse wheel)
    let mut new_zoom = zoom;
    let mut new_scroll = scroll_offset;
    if response.hovered() {
        let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll_delta != 0.0 {
            let zoom_factor = 1.0 + scroll_delta * 0.001;
            new_zoom = (new_zoom * zoom_factor).clamp(1.0, 20.0);
        }
    }

    // Handle pan (drag)
    if response.dragged() {
        let drag_delta = response.drag_delta().x;
        let bar_width = rect.width() / peaks.len() as f32;
        new_scroll = (new_scroll - drag_delta / bar_width.max(1.0) / new_zoom).max(0.0);
    }

    let bar_width = rect.width() / peaks.len() as f32 * new_zoom;
    let half_height = rect.height() / 2.0;
    let center_y = rect.center().y;

    // Compute visible bar range
    let start_bar = new_scroll as usize;
    let visible_bars = (rect.width() / bar_width.max(1.0)).ceil() as usize + 1;
    let end_bar = (start_bar + visible_bars).min(peaks.len());

    for i in start_bar..end_bar {
        let x = rect.min.x + (i as f32 - new_scroll) * bar_width;
        if x < rect.min.x - bar_width || x > rect.max.x {
            continue;
        }
        let (min_val, max_val) = &peaks[i];
        let y_top = (center_y + max_val * half_height).clamp(rect.min.y, rect.max.y);
        let y_bottom = (center_y + min_val * half_height).clamp(rect.min.y, rect.max.y);

        let bar_rect = egui::Rect::from_min_max(
            egui::pos2(x + 1.0, y_top),
            egui::pos2(x + bar_width.max(1.0), y_bottom),
        );
        painter.rect_filled(bar_rect, 0.0, Color32::from_rgb(100, 200, 100));
    }

    // Zoom scrollbar indicator
    if new_zoom > 1.0 {
        let total_virtual_width = peaks.len() as f32 * bar_width;
        let thumb_width = rect.width() / total_virtual_width * rect.width();
        let thumb_x = rect.min.x + new_scroll / peaks.len() as f32 * rect.width();
        let scrollbar_rect = egui::Rect::from_min_size(
            egui::pos2(thumb_x.clamp(rect.min.x, rect.max.x - thumb_width.max(2.0)), rect.max.y - 3.0),
            egui::vec2(thumb_width.max(2.0), 2.0),
        );
        painter.rect_filled(scrollbar_rect, 1.0, Color32::from_rgb(180, 180, 180));
    }

    (new_zoom, new_scroll)
}
