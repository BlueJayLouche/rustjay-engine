//! The "Sequencer" egui tab: click-to-toggle steps, play/stop, pattern select.

use rustjay_engine::prelude::*;

use crate::bank::{BankHandle, PAD_COUNT};

const STEPS_PER_PATTERN: usize = 16;

pub struct SequencerTab {
    _handle: BankHandle,
}

impl SequencerTab {
    pub fn new(handle: BankHandle) -> Self {
        Self { _handle: handle }
    }
}

impl AnyEguiTab for SequencerTab {
    fn name(&self) -> &str {
        "Sequencer"
    }

    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        app_state: &mut dyn std::any::Any,
        _engine: &mut EngineState,
    ) {
        let state = app_state
            .downcast_mut::<crate::Vp404State>()
            .expect("SequencerTab expects Vp404State");
        let seq = &mut state.sequencer;

        ui.horizontal(|ui| {
            let play_label = if seq.is_playing {
                "⏹ Stop"
            } else {
                "▶ Play"
            };
            if ui.button(play_label).clicked() {
                seq.toggle_playback();
            }
            if ui.button("⏵︎ Reset").clicked() {
                seq.reset_position();
            }
            if ui.button("Clear").clicked() {
                seq.clear_pattern();
            }
        });

        ui.horizontal(|ui| {
            ui.label("Pattern");
            let mut pat = seq.current_pattern;
            egui::ComboBox::from_id_salt("seq_pattern")
                .selected_text(format!("{:02}", pat + 1))
                .show_ui(ui, |ui| {
                    for i in 0..seq.patterns.len() {
                        ui.selectable_value(&mut pat, i, format!("Pattern {:02}", i + 1));
                    }
                });
            if pat != seq.current_pattern {
                seq.queue_pattern(pat);
            }
        });

        ui.separator();

        // Current playhead step (all tracks share the same clock).
        let playhead = if seq.is_playing {
            let pattern_len = seq.patterns[seq.current_pattern].length();
            ((seq.position / 0.25) as usize) % pattern_len.max(1)
        } else {
            0
        };

        // Snapshot active steps so the grid can be drawn without holding a borrow.
        let active: Vec<Vec<bool>> = seq.patterns[seq.current_pattern]
            .tracks
            .iter()
            .map(|t| {
                t.steps
                    .iter()
                    .take(STEPS_PER_PATTERN)
                    .map(|s| s.active)
                    .collect()
            })
            .collect();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for track_idx in 0..PAD_COUNT {
                let row = active.get(track_idx).map(|r| r.as_slice()).unwrap_or(&[]);
                ui.horizontal(|ui| {
                    ui.label(format!("{:02}", track_idx + 1));

                    let muted = seq.patterns[seq.current_pattern].tracks[track_idx].muted;
                    let mute_label = if muted { "🔇" } else { "M" };
                    if ui.button(mute_label).clicked() {
                        if muted {
                            seq.unmute_track(track_idx);
                        } else {
                            seq.mute_track(track_idx);
                        }
                    }

                    for step_idx in 0..STEPS_PER_PATTERN {
                        let is_active = row.get(step_idx).copied().unwrap_or(false);
                        let is_playhead = step_idx == playhead;

                        let size = egui::vec2(18.0, 22.0);
                        let fill = if is_active {
                            egui::Color32::from_rgb(100, 220, 255)
                        } else {
                            egui::Color32::from_gray(40)
                        };
                        let stroke = if is_playhead {
                            egui::Stroke::new(2.0, egui::Color32::WHITE)
                        } else {
                            egui::Stroke::NONE
                        };
                        let resp = ui.add(
                            egui::Button::new("")
                                .fill(fill)
                                .min_size(size)
                                .stroke(stroke),
                        );
                        if resp.clicked() {
                            seq.toggle_step(track_idx, step_idx);
                        }
                    }
                });
            }
        });
    }
}
