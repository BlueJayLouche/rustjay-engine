//! Waveform display — generates and renders audio peak data.
//!
//! Peak files (`.qpek`) cache decoded waveform data for instant reload.

use egui::Color32;
use cuepool_audio::SampleProvider;
use std::io::{Read, Write};

const QPEK_MAGIC: &[u8] = b"QPEK";
const QPEK_VERSION: u32 = 1;

#[derive(Debug, Clone, Default)]
pub struct WaveformData {
    pub peaks: Vec<(f32, f32)>,
    pub duration_secs: f32,
}

/// Generate or load cached peak data for a waveform.
/// Returns `num_bars` pairs of (min, max) sample values in [-1, 1].
pub fn generate_peaks(path: &str, num_bars: usize) -> Option<WaveformData> {
    let decoder = cuepool_audio::FileDecoder::open(path).ok()?;
    let length = decoder.length()?;
    if length == 0 || num_bars == 0 {
        return None;
    }
    let duration_secs = length as f32
        / f32::from(decoder.channels().max(1))
        / decoder.sample_rate().max(1) as f32;

    // Opening the decoder is intentionally still required for a cached peak file:
    // the sidecar predates scrubbing and does not store the media duration.
    if let Some(peaks) = load_peaks(path) {
        return Some(WaveformData { peaks, duration_secs });
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

    Some(WaveformData { peaks, duration_secs })
}

/// Load peaks from a `.qpek` sidecar file if it exists and is valid.
fn load_peaks(audio_path: &str) -> Option<Vec<(f32, f32)>> {
    let peak_path = format!("{}.qpek", audio_path);
    let mut file = std::fs::File::open(&peak_path).ok()?;

    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).ok()?;
    if magic != QPEK_MAGIC {
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct Playhead {
    pub(crate) position_secs: f32,
    pub(crate) region_start_secs: f32,
    pub(crate) seek_length_secs: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Interaction {
    Disabled,
    Pan,
    Scrub(crate::scrub::SeekKind),
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DrawResponse {
    pub(crate) zoom: f32,
    pub(crate) scroll_offset: f32,
    pub(crate) seek_target: Option<f32>,
}

/// Convert a pointer x relative to the waveform into whole-media seconds.
pub(crate) fn pointer_x_to_media_secs(
    x: f32,
    width: f32,
    peak_count: usize,
    zoom: f32,
    scroll_offset: f32,
    media_duration_secs: f32,
) -> f32 {
    if width <= 0.0 || peak_count == 0 || media_duration_secs <= 0.0 {
        return 0.0;
    }
    let bar_width = width / peak_count as f32 * zoom.max(1.0);
    let bar = scroll_offset.max(0.0) + x.clamp(0.0, width) / bar_width;
    (bar / peak_count as f32).clamp(0.0, 1.0) * media_duration_secs
}

/// Convert whole-media seconds into the cue-relative `SeekCue` timeline.
pub(crate) fn media_to_seek_secs(
    media_secs: f32,
    region_start_secs: f32,
    seek_length_secs: f32,
) -> f32 {
    (media_secs.max(0.0) - region_start_secs.max(0.0))
        .clamp(0.0, seek_length_secs.max(0.0))
}

fn media_secs_to_x(
    media_secs: f32,
    rect: egui::Rect,
    peak_count: usize,
    zoom: f32,
    scroll_offset: f32,
    media_duration_secs: f32,
) -> f32 {
    let bar_width = rect.width() / peak_count as f32 * zoom;
    let bar = media_secs.clamp(0.0, media_duration_secs) / media_duration_secs * peak_count as f32;
    rect.min.x + (bar - scroll_offset) * bar_width
}

fn clamp_scroll_offset(scroll_offset: f32, peak_count: usize, zoom: f32) -> f32 {
    if !scroll_offset.is_finite() || peak_count == 0 {
        return 0.0;
    }
    let zoom = zoom.max(1.0);
    let max_scroll = peak_count as f32 * (1.0 - 1.0 / zoom);
    scroll_offset.clamp(0.0, max_scroll)
}

fn panning_button(interaction: Interaction, primary: bool, secondary: bool) -> bool {
    match interaction {
        Interaction::Disabled => false,
        Interaction::Pan => primary || secondary,
        Interaction::Scrub(_) => secondary,
    }
}

/// Draw a waveform from pre-computed whole-media data with zoom and pan support.
/// Primary-drag pans unless an editable playhead is supplied; secondary-drag
/// remains available for panning while scrubbing is enabled.
pub(crate) fn draw(
    ui: &mut egui::Ui,
    waveform: &WaveformData,
    zoom: f32,
    scroll_offset: f32,
    height: f32,
    interaction: Interaction,
    playhead: Option<Playhead>,
) -> DrawResponse {
    let desired_size = egui::vec2(ui.available_width(), height);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
    let painter = ui.painter();

    painter.rect_filled(rect, 2.0, Color32::from_rgb(30, 30, 30));
    if waveform.peaks.is_empty() || waveform.duration_secs <= 0.0 {
        return DrawResponse { zoom, scroll_offset, seek_target: None };
    }

    let mut new_zoom = zoom;
    let mut new_scroll = scroll_offset;
    if response.hovered() {
        let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll_delta != 0.0 {
            let zoom_factor = 1.0 + scroll_delta * 0.001;
            new_zoom = (new_zoom * zoom_factor).clamp(1.0, 20.0);
        }
    }

    let panning = panning_button(
        interaction,
        response.dragged_by(egui::PointerButton::Primary),
        response.dragged_by(egui::PointerButton::Secondary),
    );
    if panning {
        // `Response::drag_delta` is cumulative for the whole gesture. The
        // persisted scroll value needs only this frame's pointer movement,
        // otherwise a long drag accelerates until the waveform disappears.
        let drag_delta = ui.input(|input| input.pointer.delta().x);
        let bar_width = rect.width() / waveform.peaks.len() as f32;
        new_scroll = (new_scroll - drag_delta / bar_width.max(1.0) / new_zoom).max(0.0);
    }
    new_scroll = clamp_scroll_offset(new_scroll, waveform.peaks.len(), new_zoom);

    let pointer_media_secs = response.interact_pointer_pos().map(|pointer| {
        pointer_x_to_media_secs(
            pointer.x - rect.min.x,
            rect.width(),
            waveform.peaks.len(),
            new_zoom,
            new_scroll,
            waveform.duration_secs,
        )
    });
    let drag_update = playhead
        .and_then(|playhead| match interaction {
            Interaction::Scrub(kind) => Some({
                let pointer_target = pointer_media_secs.map(|media_secs| {
                    media_to_seek_secs(
                        media_secs,
                        playhead.region_start_secs,
                        playhead.seek_length_secs,
                    )
                });
                crate::scrub::update_drag(
                    ui,
                    response.id.with("scrub"),
                    &response,
                    pointer_target,
                    kind,
                )
            }),
            Interaction::Disabled | Interaction::Pan => None,
        })
        .unwrap_or_default();
    if matches!(interaction, Interaction::Scrub(_)) {
        let _ = response.clone().on_hover_and_drag_cursor(egui::CursorIcon::ResizeHorizontal);
        response.widget_info(|| {
            egui::WidgetInfo::slider(true, 0.0, "Scrub selected cue waveform")
        });
    }

    let bar_width = rect.width() / waveform.peaks.len() as f32 * new_zoom;
    let half_height = rect.height() / 2.0;
    let center_y = rect.center().y;
    let start_bar = new_scroll as usize;
    let visible_bars = (rect.width() / bar_width.max(1.0)).ceil() as usize + 1;
    let end_bar = (start_bar + visible_bars).min(waveform.peaks.len());

    for (i, (min_val, max_val)) in waveform.peaks.iter().enumerate().take(end_bar).skip(start_bar) {
        let x = rect.min.x + (i as f32 - new_scroll) * bar_width;
        if x < rect.min.x - bar_width || x > rect.max.x {
            continue;
        }
        let y_top = (center_y + max_val * half_height).clamp(rect.min.y, rect.max.y);
        let y_bottom = (center_y + min_val * half_height).clamp(rect.min.y, rect.max.y);
        let bar_rect = egui::Rect::from_min_max(
            egui::pos2(x + 1.0, y_top),
            egui::pos2(x + bar_width.max(1.0), y_bottom),
        );
        painter.rect_filled(bar_rect, 0.0, Color32::from_rgb(100, 200, 100));
    }

    if new_zoom > 1.0 {
        let total_virtual_width = waveform.peaks.len() as f32 * bar_width;
        let thumb_width = rect.width() / total_virtual_width * rect.width();
        let thumb_x = rect.min.x + new_scroll / waveform.peaks.len() as f32 * rect.width();
        let scrollbar_rect = egui::Rect::from_min_size(
            egui::pos2(thumb_x.clamp(rect.min.x, rect.max.x - thumb_width.max(2.0)), rect.max.y - 3.0),
            egui::vec2(thumb_width.max(2.0), 2.0),
        );
        painter.rect_filled(scrollbar_rect, 1.0, Color32::from_rgb(180, 180, 180));
    }

    if let Some(playhead) = playhead {
        let cue_position = drag_update.preview_target.unwrap_or(playhead.position_secs);
        let cue_position = if playhead.seek_length_secs > 0.0 {
            cue_position.clamp(0.0, playhead.seek_length_secs)
        } else {
            cue_position.max(0.0)
        };
        let x = media_secs_to_x(
            playhead.region_start_secs + cue_position,
            rect,
            waveform.peaks.len(),
            new_zoom,
            new_scroll,
            waveform.duration_secs,
        );
        if rect.x_range().contains(x) {
            let segment = [egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)];
            painter.line_segment(segment, egui::Stroke::new(3.0_f32, Color32::from_rgb(25, 25, 25)));
            painter.line_segment(segment, egui::Stroke::new(1.0_f32, Color32::WHITE));
        }
    }

    DrawResponse {
        zoom: new_zoom,
        scroll_offset: new_scroll,
        seek_target: drag_update.emit_target,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_x_maps_through_waveform_zoom_and_scroll() {
        assert_eq!(pointer_x_to_media_secs(0.0, 400.0, 200, 2.0, 50.0, 120.0), 30.0);
        assert_eq!(pointer_x_to_media_secs(200.0, 400.0, 200, 2.0, 50.0, 120.0), 60.0);
        assert_eq!(pointer_x_to_media_secs(400.0, 400.0, 200, 2.0, 50.0, 120.0), 90.0);
    }

    #[test]
    fn media_time_maps_to_the_clamped_loop_timeline() {
        assert_eq!(media_to_seek_secs(25.0, 30.0, 20.0), 0.0);
        assert_eq!(media_to_seek_secs(37.5, 30.0, 20.0), 7.5);
        assert_eq!(media_to_seek_secs(55.0, 30.0, 20.0), 20.0);
    }

    #[test]
    fn waveform_pan_stays_within_the_visible_peak_range() {
        assert_eq!(clamp_scroll_offset(-10.0, 200, 2.0), 0.0);
        assert_eq!(clamp_scroll_offset(250.0, 200, 2.0), 100.0);
        assert_eq!(clamp_scroll_offset(50.0, 200, 1.0), 0.0);
    }

    #[test]
    fn disabled_waveform_rejects_both_drag_buttons() {
        assert!(!panning_button(Interaction::Disabled, true, false));
        assert!(!panning_button(Interaction::Disabled, false, true));
        assert!(panning_button(Interaction::Pan, true, false));
        assert!(panning_button(
            Interaction::Scrub(crate::scrub::SeekKind::Sound),
            false,
            true,
        ));
    }
}
