//! Transport controls — Go, Stop, Pause buttons.

use crate::app::{AppCommand, GuiMeterData, SharedStateHandle};
use crate::colour_to_egui;
use egui::{Button, Color32, RichText, Vec2};
use rust_decimal::Decimal;

fn standby_label(standby: Option<Decimal>) -> String {
    let Some(qid) = standby else {
        return "Standby: (no cue selected)".to_string();
    };
    format!("Standby: Q{qid}")
}

/// `HH:MM:SS.ff` at a display-only frame rate (triggers store seconds).
pub(crate) fn format_timecode(secs: f64, fps: f32) -> String {
    let fps = fps.max(1.0) as f64;
    let t = secs.max(0.0);
    let frames = (t * fps).round() as u64;
    let fph = (fps * 3600.0) as u64;
    let fpm = (fps * 60.0) as u64;
    let fpsu = fps as u64;
    format!(
        "{:02}:{:02}:{:02}.{:02}",
        frames / fph,
        (frames % fph) / fpm,
        (frames % fpm) / fpsu,
        frames % fpsu
    )
}

/// Parse a timecode string into seconds. Colon-separated `[HH:][MM:]SS[.FF]`
/// where `.FF` is a frame count at `fps` (matching `format_timecode`); a bare
/// number without colons is plain seconds. Returns `None` on malformed input.
pub(crate) fn parse_timecode(text: &str, fps: f32) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if !t.contains(':') {
        return t.parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.0);
    }
    let (hms, frames) = match t.rsplit_once('.') {
        Some((a, f)) => (a, f.trim().parse::<u32>().ok()? as f64),
        None => (t, 0.0),
    };
    let mut secs = 0.0;
    for part in hms.split(':') {
        secs = secs * 60.0 + part.trim().parse::<u32>().ok()? as f64;
    }
    Some(secs + frames / fps.max(1.0) as f64)
}

pub fn show(ui: &mut egui::Ui, state: &SharedStateHandle) {
    let standby = state.lock().ok().and_then(|state| {
        state.selected_cue().map(|cue| {
            let base = cue.base();
            (base.qid, base.name.clone(), base.colour)
        })
    });
    let has_standby = standby.is_some();

    let controls = |ui: &mut egui::Ui| {
        let button_size = Vec2::new(60.0, 32.0);

        let go_btn = Button::new(RichText::new("▶ GO").strong().color(Color32::WHITE))
            .fill(Color32::from_rgb(0, 180, 0))
            .min_size(button_size);
        let go_hover = standby.as_ref().map_or_else(
            || "Select a cue before using Go".to_string(),
            |(qid, name, _)| format!("Fire Q{qid} {name} (Space)"),
        );
        if ui
            .add_enabled(has_standby, go_btn)
            .on_hover_text(go_hover)
            .clicked()
            && let Ok(mut state) = state.lock()
        {
            state.command_queue.push(AppCommand::Go);
        }

        let readout = standby_label(standby.as_ref().map(|(qid, _, _)| *qid));
        let readout_hover = standby.as_ref().map_or_else(
            || "Select a cue to set the standby playhead".to_string(),
            |(qid, name, _)| format!("Standby: Q{qid} {name}. Go fires this cue."),
        );
        let readout_width = ui.available_width().min(190.0);
        ui.allocate_ui_with_layout(
            Vec2::new(readout_width, 32.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                if let Some((_, _, colour)) = &standby {
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 3.0, colour_to_egui(*colour));
                    response.on_hover_text("Standby cue colour tag");
                }
                let text = if has_standby {
                    RichText::new(readout).strong()
                } else {
                    RichText::new(readout).weak()
                };
                ui.label(text);
                if let Some((_, name, _)) = &standby {
                    ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
                }
            },
        )
        .response
        .on_hover_text(readout_hover);

        let stop_btn = Button::new(RichText::new("⏹ STOP").strong())
            .fill(Color32::from_rgb(200, 0, 0))
            .min_size(button_size);
        if ui
            .add(stop_btn)
            .on_hover_text("Stop all cues (Esc)")
            .clicked()
            && let Ok(mut state) = state.lock()
        {
            state.command_queue.push(AppCommand::Stop);
        }

        let pause_btn = Button::new(RichText::new("⏸ PAUSE")).min_size(button_size);
        if ui
            .add(pause_btn)
            .on_hover_text("Pause/resume the show clock")
            .clicked()
            && let Ok(mut state) = state.lock()
        {
            state.command_queue.push(AppCommand::Pause);
        }

        let (show_time, show_paused, next_tc, tc_fps) = {
            let Ok(state) = state.lock() else { return };
            (
                state.show_time,
                state.show_paused,
                state.next_timecode,
                state.show_file.show_settings.timecode_fps,
            )
        };
        let (mtc_running, mtc_playing, mtc_secs, mtc_fps, mtc_source, mtc_drift_ms) = {
            let Ok(state) = state.lock() else { return };
            (
                state.mtc_running,
                state.mtc_playing,
                state.mtc_timecode_secs,
                state.mtc_fps,
                state.mtc_source.clone(),
                state.mtc_drift_ms,
            )
        };
        let back_btn = Button::new(RichText::new("⏮")).min_size(Vec2::new(36.0, 32.0));
        if ui
            .add_enabled(show_paused, back_btn)
            .on_hover_text("Step one video frame back (paused only); the show clock follows")
            .clicked()
            && let Ok(mut state) = state.lock()
        {
            state.command_queue.push(AppCommand::FrameStepBack);
        }
        let step_btn = Button::new(RichText::new("⏭")).min_size(Vec2::new(36.0, 32.0));
        if ui
            .add_enabled(show_paused, step_btn)
            .on_hover_text("Step one video frame forward (paused only); the show clock follows")
            .clicked()
            && let Ok(mut state) = state.lock()
        {
            state.command_queue.push(AppCommand::FrameStep);
        }

        let preload_btn = Button::new(RichText::new("PRELOAD")).min_size(Vec2::new(70.0, 32.0));
        if ui
            .add_enabled(has_standby, preload_btn)
            .on_hover_text(if has_standby {
                "Load the standby cue's media into memory so Go starts instantly"
            } else {
                "Select a cue before preloading"
            })
            .clicked()
            && let Ok(mut state) = state.lock()
        {
            state.command_queue.push(AppCommand::Preload);
        }

        ui.separator();

        // Show clock (the clock timecode triggers fire against) + next trigger.
        let tc_color = if show_paused {
            Color32::from_rgb(230, 190, 60)
        } else if show_time.is_some() {
            Color32::from_rgb(120, 220, 120)
        } else {
            Color32::from_gray(110)
        };
        let tc_text = match show_time {
            Some(t) => format_timecode(t, tc_fps),
            None => "--:--:--.--".into(),
        };
        ui.label(
            RichText::new(tc_text)
                .monospace()
                .size(20.0)
                .color(tc_color),
        )
        .on_hover_text(if show_paused {
            "Show clock (paused)"
        } else {
            "Show clock"
        });
        if let Some((qid, t)) = next_tc {
            ui.label(
                RichText::new(format!("next: Q{qid} @ {}", format_timecode(t, tc_fps)))
                    .monospace()
                    .small()
                    .weak(),
            )
            .on_hover_text("Next armed timecode trigger");
        }

        // MTC readout (timecode from e.g. Pro Tools over RTP-MIDI). Shown once
        // an MTC source has been seen; green while the transport is playing.
        if mtc_running || !mtc_source.is_empty() {
            ui.separator();
            let mtc_color = if mtc_playing {
                Color32::from_rgb(120, 220, 120)
            } else {
                Color32::from_gray(110)
            };
            ui.label(
                RichText::new(format!("MTC {}", format_timecode(mtc_secs, mtc_fps as f32)))
                    .monospace()
                    .size(20.0)
                    .color(mtc_color),
            )
            .on_hover_text(if mtc_playing {
                "MIDI timecode (playing)"
            } else {
                "MIDI timecode (stopped)"
            });
            if !mtc_source.is_empty() {
                ui.label(RichText::new(&mtc_source).small().weak());
            }
            if let Some(drift) = mtc_drift_ms {
                ui.label(
                    RichText::new(format!("drift {:+.0}ms", drift))
                        .monospace()
                        .small()
                        .weak(),
                )
                .on_hover_text("MTC target − video position (follow cue active)");
            }
        }

        ui.separator();

        // Show / Edit mode toggle
        let mode = {
            let Ok(state) = state.lock() else { return };
            state.show_mode
        };

        let mode_label = match mode {
            crate::app::ShowMode::Edit => "Edit Mode",
            crate::app::ShowMode::Show => "Show Mode",
        };
        let mode_color = match mode {
            crate::app::ShowMode::Edit => Color32::from_rgb(60, 60, 60),
            crate::app::ShowMode::Show => Color32::from_rgb(180, 140, 0),
        };

        let mode_btn = Button::new(RichText::new(mode_label).strong().color(Color32::WHITE))
            .fill(mode_color)
            .min_size(Vec2::new(100.0, 32.0));
        if ui
            .add(mode_btn)
            .on_hover_text("Toggle Show/Edit mode — cue editing is locked in Show mode")
            .clicked()
            && let Ok(mut state) = state.lock()
        {
            let snapshot = crate::app::Snapshot::from_state(&state);
            state.undo_redo.push(snapshot);
            state.show_mode = match state.show_mode {
                crate::app::ShowMode::Edit => crate::app::ShowMode::Show,
                crate::app::ShowMode::Show => crate::app::ShowMode::Edit,
            };
            state.dirty = true;
        }
    };

    egui::containers::Sides::new()
        .height(32.0)
        .shrink_left()
        .truncate()
        .show(ui, controls, |ui| {
            // Reserve the right edge for the master meter so the bounded standby
            // readout cannot push it off-screen in a narrow window.
            let meter_data = {
                let Ok(state) = state.lock() else { return };
                state.meter_data
            };
            draw_meter(ui, &meter_data);
        });
}

fn draw_meter(ui: &mut egui::Ui, data: &GuiMeterData) {
    let width = 8.0;
    let height = 32.0;
    let _gap = 4.0;

    for &(peak_db, rms_db) in &[
        (data.peak_l_db, data.rms_l_db),
        (data.peak_r_db, data.rms_r_db),
    ] {
        let (rect, _response) =
            ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
        let painter = ui.painter();

        // Background
        painter.rect_filled(rect, 1.0, Color32::from_rgb(30, 30, 30));

        // Draw segments from bottom up
        let segments = 12i32;
        let seg_h = height / segments as f32;
        for i in 0..segments {
            let seg_db = -60.0 + (i as f32 / segments as f32) * 60.0; // -60dB to 0dB
            let seg_y = rect.max.y - (i as f32 + 0.5) * seg_h;
            let seg_rect = egui::Rect::from_center_size(
                egui::pos2(rect.center().x, seg_y),
                egui::vec2(width - 2.0, seg_h - 1.0),
            );

            let lit = rms_db >= seg_db || peak_db >= seg_db;
            let peak_lit = peak_db >= seg_db;
            let colour = if seg_db >= 0.0 {
                Color32::RED
            } else if seg_db >= -12.0 {
                Color32::YELLOW
            } else {
                Color32::GREEN
            };

            if peak_lit {
                painter.rect_filled(seg_rect, 1.0, colour);
            } else if lit {
                painter.rect_filled(seg_rect, 1.0, colour.gamma_multiply(0.5));
            }
        }
    }

    // GR meter (gain reduction)
    let (rect, _response) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 1.0, Color32::from_rgb(30, 30, 30));

    let gr_segments = 12i32;
    let seg_h = height / gr_segments as f32;
    let gr_db = data.limiter_gr_db;
    for i in 0..gr_segments {
        let seg_db = -(i as f32 / gr_segments as f32) * 30.0; // 0 to -30 dB
        let seg_y = rect.min.y + (i as f32 + 0.5) * seg_h;
        let seg_rect = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, seg_y),
            egui::vec2(width - 2.0, seg_h - 1.0),
        );
        let lit = gr_db <= seg_db;
        let colour = if seg_db <= -20.0 {
            Color32::RED
        } else if seg_db <= -10.0 {
            Color32::YELLOW
        } else {
            Color32::from_rgb(100, 200, 255)
        };
        if lit {
            painter.rect_filled(seg_rect, 1.0, colour);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{format_timecode, parse_timecode, standby_label};
    use rust_decimal::Decimal;

    #[test]
    fn standby_labels_cover_empty_and_selected_states() {
        assert_eq!(standby_label(None), "Standby: (no cue selected)");
        assert_eq!(standby_label(Some(Decimal::new(35, 1))), "Standby: Q3.5");
    }

    #[test]
    fn timecode_formatting() {
        assert_eq!(format_timecode(0.0, 30.0), "00:00:00.00");
        assert_eq!(format_timecode(1.0, 30.0), "00:00:01.00");
        assert_eq!(format_timecode(0.5, 30.0), "00:00:00.15");
        assert_eq!(
            format_timecode(3661.5, 25.0),
            "01:01:01.13",
            "25fps half-second = frame 12.5 → 13"
        );
        assert_eq!(
            format_timecode(-3.0, 30.0),
            "00:00:00.00",
            "negative clamps"
        );
    }

    #[test]
    fn timecode_parsing() {
        assert_eq!(
            parse_timecode("00:19:28.08", 25.0),
            Some(19.0 * 60.0 + 28.0 + 8.0 / 25.0)
        );
        assert_eq!(parse_timecode("01:01:01", 30.0), Some(3661.0));
        assert_eq!(parse_timecode("2:30", 30.0), Some(150.0), "MM:SS shorthand");
        assert_eq!(
            parse_timecode("2:30.15", 30.0),
            Some(150.5),
            "frames at 30fps"
        );
        assert_eq!(
            parse_timecode("668.259", 25.0),
            Some(668.259),
            "no colons = plain seconds"
        );
        assert_eq!(parse_timecode("", 30.0), None);
        assert_eq!(parse_timecode("abc", 30.0), None);
        assert_eq!(parse_timecode("1:xx", 30.0), None);
        assert_eq!(parse_timecode("-5", 30.0), None, "negative rejected");
        // Round-trip: format(parse(x)) == x
        for s in ["00:00:00.00", "00:19:28.08", "12:34:56.20"] {
            let secs = parse_timecode(s, 25.0).unwrap();
            assert_eq!(format_timecode(secs, 25.0), s);
        }
    }
}
