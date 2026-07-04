//! Active cues panel — left sidebar showing currently playing cues.

use crate::app::SharedStateHandle;
use egui::{Color32, RichText};

/// `m:ss.t` (minutes, seconds, tenths). Rounds to tenths first so 59.97s
/// renders as 1:00.0, not 0:60.0.
fn fmt_time(secs: f32) -> String {
    let tenths = (secs.max(0.0) * 10.0).round() as u64;
    format!("{}:{:04.1}", tenths / 600, (tenths % 600) as f64 / 10.0)
}

pub fn show(ui: &mut egui::Ui, state: &SharedStateHandle) {
    let active_cues = {
        let Ok(state) = state.lock() else { return };
        state.active_cues.clone()
    };

    ui.heading("Active Cues");
    ui.separator();

    if active_cues.is_empty() {
        ui.label(RichText::new("No active cues").italics().color(Color32::GRAY));
        return;
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        for cue in &active_cues {
            let qid_str = cue.qid.to_string();
            let db = if cue.volume > 0.0 {
                20.0 * cue.volume.log10()
            } else {
                -f32::INFINITY
            };

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

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if cue.paused {
                                    ui.colored_label(Color32::YELLOW, "⏸");
                                }

                                // Tiny volume meter
                                let meter_width = 40.0;
                                let meter_height = 12.0;
                                let (rect, _response) = ui.allocate_exact_size(
                                    egui::vec2(meter_width, meter_height),
                                    egui::Sense::hover(),
                                );
                                let bg = Color32::from_rgb(40, 40, 40);
                                ui.painter().rect_filled(rect, 2.0, bg);

                                let norm = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
                                let fill_width = meter_width * norm;
                                if fill_width > 0.0 {
                                    let fill_rect = egui::Rect::from_min_size(
                                        rect.min,
                                        egui::vec2(fill_width, meter_height),
                                    );
                                    let colour = if db > 0.0 {
                                        Color32::RED
                                    } else if db > -12.0 {
                                        Color32::YELLOW
                                    } else {
                                        Color32::GREEN
                                    };
                                    ui.painter().rect_filled(fill_rect, 2.0, colour);
                                }
                            });
                        });

                        // Playback progress bar: elapsed / total, remaining on the right.
                        if let Some(length) = cue.length_secs.filter(|l| *l > 0.0) {
                            let progress = (cue.position_secs / length).clamp(0.0, 1.0);
                            let remaining = (length - cue.position_secs).max(0.0);
                            let fill = if cue.paused {
                                Color32::from_rgb(200, 170, 50)
                            } else {
                                Color32::from_rgb(100, 180, 100)
                            };
                            ui.add(
                                egui::ProgressBar::new(progress)
                                    .desired_height(14.0)
                                    .fill(fill)
                                    .text(
                                        RichText::new(format!(
                                            "{} / {}  −{}",
                                            fmt_time(cue.position_secs),
                                            fmt_time(length),
                                            fmt_time(remaining),
                                        ))
                                        .monospace()
                                        .size(10.0),
                                    ),
                            );
                        }
                    });
                });
        }
    });
}
