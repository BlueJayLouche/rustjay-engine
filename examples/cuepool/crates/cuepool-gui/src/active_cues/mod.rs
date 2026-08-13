//! Active cues panel — left sidebar showing currently playing cues.

use crate::app::{ActiveCueInfo, AppCommand, SharedStateHandle, ShowMode};
use egui::{Color32, RichText};

/// `m:ss.t` (minutes, seconds, tenths). Rounds to tenths first so 59.97s
/// renders as 1:00.0, not 0:60.0.
fn fmt_time(secs: f32) -> String {
    let tenths = (secs.max(0.0) * 10.0).round() as u64;
    format!("{}:{:04.1}", tenths / 600, (tenths % 600) as f64 / 10.0)
}

pub fn show(ui: &mut egui::Ui, state: &SharedStateHandle) {
    let (active_cues, show_mode, seek_kinds) = {
        let Ok(state) = state.lock() else { return };
        let seek_kinds = state.show_file.cues.iter().filter_map(|cue| {
            let kind = match cue {
                cuepool_core::Cue::Sound { .. } => crate::scrub::SeekKind::Sound,
                cuepool_core::Cue::Video { .. } => crate::scrub::SeekKind::Video,
                _ => return None,
            };
            Some((cue.base().qid, kind))
        }).collect::<std::collections::HashMap<_, _>>();
        (state.active_cues.clone(), state.show_mode, seek_kinds)
    };
    let mut pending_commands = Vec::new();

    ui.heading("Active Cues");
    ui.separator();

    if active_cues.is_empty() {
        ui.label(RichText::new("No active cues").italics().color(Color32::GRAY));
        return;
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        for cue in &active_cues {
            let qid_str = cue.qid.to_string();

            egui::Frame::new()
                .fill(ui.visuals().panel_fill)
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_min_height(24.0);

                        ui.horizontal(|ui| {
                            // State indicator
                            let state_icon = match cue.state {
                                crate::app::CueState::Ready => "○",
                                crate::app::CueState::Delay => "◐",
                                crate::app::CueState::Playing => "▶",
                                crate::app::CueState::PlayingLooped => "🔁",
                                crate::app::CueState::Paused => "⏸",
                                crate::app::CueState::Done => "✓",
                            };
                            ui.label(RichText::new(state_icon).monospace().size(12.0));

                            // Q# + name
                            let label = format!("Q{}  {}", qid_str, cue.name);
                            let mut text = RichText::new(label).monospace().size(12.0);
                            if cue.paused {
                                text = text.color(Color32::YELLOW);
                            }
                            ui.label(text);

                            if cue.paused {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.colored_label(Color32::YELLOW, "⏸");
                                });
                            }
                        });

                        // Playback progress bar: elapsed / total, remaining on the right.
                        if let Some(length) = cue.length_secs.filter(|l| *l > 0.0) {
                            let fill = if cue.paused {
                                Color32::from_rgb(200, 170, 50)
                            } else {
                                Color32::from_rgb(100, 180, 100)
                            };
                            let kind = if show_mode == ShowMode::Edit {
                                seek_kinds.get(&cue.qid).copied()
                            } else {
                                None
                            };
                            if let Some(secs) = draw_progress_bar(ui, cue, length, fill, kind) {
                                pending_commands.push(AppCommand::SeekCue {
                                    instance_id: cue.instance_id,
                                    secs,
                                });
                            }
                        }
                    });
                });
        }
    });

    if !pending_commands.is_empty()
        && let Ok(mut state) = state.lock() {
            state.command_queue.extend(pending_commands);
        }
}

fn draw_progress_bar(
    ui: &mut egui::Ui,
    cue: &ActiveCueInfo,
    length: f32,
    fill: Color32,
    kind: Option<crate::scrub::SeekKind>,
) -> Option<f32> {
    let sense = if kind.is_some() { egui::Sense::click_and_drag() } else { egui::Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(96.0), 14.0),
        sense,
    );
    let pointer_target = response.interact_pointer_pos().map(|pointer| {
        ((pointer.x - rect.min.x) / rect.width()).clamp(0.0, 1.0) * length
    });
    let drag_update = kind.map(|kind| {
        crate::scrub::update_drag(
            ui,
            response.id.with("scrub"),
            &response,
            pointer_target,
            kind,
        )
    }).unwrap_or_default();
    let position_secs = drag_update.preview_target.unwrap_or(cue.position_secs);
    let progress = (position_secs / length).clamp(0.0, 1.0);
    let remaining = (length - position_secs).max(0.0);

    if kind.is_some() {
        let _ = response.clone().on_hover_and_drag_cursor(egui::CursorIcon::ResizeHorizontal);
        response.widget_info(|| {
            egui::WidgetInfo::slider(
                true,
                position_secs as f64,
                format!("Scrub active cue Q{}", cue.qid),
            )
        });
    } else {
        response.widget_info(|| egui::WidgetInfo::labeled(
            egui::WidgetType::ProgressIndicator,
            true,
            format!("Active cue Q{} progress", cue.qid),
        ));
    }

    let painter = ui.painter();
    let corner_radius = rect.height() / 2.0;
    painter.rect_filled(rect, corner_radius, ui.visuals().extreme_bg_color);
    let fill_rect = egui::Rect::from_min_size(
        rect.min,
        egui::vec2((rect.width() * progress).max(rect.height()), rect.height()),
    );
    painter.rect_filled(fill_rect, corner_radius, fill);
    painter.text(
        egui::pos2(rect.min.x + ui.spacing().item_spacing.x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!(
            "{} / {}  −{}",
            fmt_time(position_secs),
            fmt_time(length),
            fmt_time(remaining),
        ),
        egui::FontId::monospace(10.0),
        ui.visuals().selection.stroke.color,
    );

    let x = (rect.min.x + rect.width() * progress).clamp(rect.min.x + 1.0, rect.max.x - 1.0);
    let segment = [egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)];
    painter.line_segment(segment, egui::Stroke::new(3.0_f32, Color32::from_rgb(25, 25, 25)));
    painter.line_segment(segment, egui::Stroke::new(1.0_f32, Color32::WHITE));

    drag_update.emit_target
}
