//! The "Sequencer" egui tab: click-to-toggle steps, play/stop, pattern select.

use rustjay_engine::prelude::*;

use crate::bank::{BankHandle, PAD_COUNT};

const STEPS_PER_ROW: usize = 16;

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

        // Note: the Space-key play/pause shortcut is handled in the plugin's
        // `prepare()` (single consumer, no render-pass race). This tab only drives
        // the on-screen buttons.

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

            ui.separator();

            ui.label("Length");
            let mut len = seq.patterns[seq.current_pattern].length();
            egui::ComboBox::from_id_salt("seq_length")
                .selected_text(format!("{len}"))
                .show_ui(ui, |ui| {
                    for steps in [16usize, 32, 48, 64] {
                        if ui.selectable_value(&mut len, steps, format!("{steps}")).clicked() {
                            seq.patterns[seq.current_pattern].set_length(steps);
                        }
                    }
                });
        });

        ui.separator();
        ui.label(
            egui::RichText::new("Click a step to toggle · drag a step right to extend its gate (tie)")
                .size(11.0)
                .weak(),
        );

        let pattern_len = seq.patterns[seq.current_pattern].length();

        // Current playhead step (all tracks share the same clock).
        let playhead = if seq.is_playing {
            ((seq.position / 0.25) as usize) % pattern_len.max(1)
        } else {
            0
        };

        // Snapshot (active, gate_length) so lanes can be painted without holding
        // a borrow on `seq` while we also mutate it on click/drag.
        let lanes: Vec<Vec<(bool, f32)>> = seq.patterns[seq.current_pattern]
            .tracks
            .iter()
            .map(|t| {
                t.steps
                    .iter()
                    .take(pattern_len)
                    .map(|s| (s.active, s.gate_length))
                    .collect()
            })
            .collect();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for track_idx in 0..PAD_COUNT {
                let lane = lanes.get(track_idx).map(|r| r.as_slice()).unwrap_or(&[]);
                let muted = seq.patterns[seq.current_pattern].tracks[track_idx].muted;
                let n_rows = pattern_len.div_ceil(STEPS_PER_ROW);

                for sub_row in 0..n_rows {
                    ui.horizontal(|ui| {
                        if sub_row == 0 {
                            ui.label(format!("{:02}", track_idx + 1));
                            let mute_label = if muted { "🔇" } else { "M" };
                            if ui.button(mute_label).clicked() {
                                if muted {
                                    seq.unmute_track(track_idx);
                                } else {
                                    seq.mute_track(track_idx);
                                }
                            }
                        } else {
                            // Align subsequent rows under the step lane.
                            ui.add_space(42.0);
                        }

                        let start = sub_row * STEPS_PER_ROW;
                        let end = (start + STEPS_PER_ROW).min(pattern_len);
                        if let Some((toggle, extend)) = step_lane(
                            ui, track_idx, start, end, playhead, lane,
                        ) {
                            if let Some(idx) = toggle {
                                seq.toggle_step(track_idx, idx);
                                // A freshly-activated step gets a full one-step gate.
                                if seq.current_pattern().tracks[track_idx].steps[idx].active {
                                    if let Some(t) =
                                        seq.current_pattern_mut().get_track_mut(track_idx)
                                    {
                                        if let Some(s) = t.steps.get_mut(idx) {
                                            s.gate_length = 1.0;
                                        }
                                    }
                                }
                            }
                            if let Some((idx, gate)) = extend {
                                seq.set_step(track_idx, idx, true);
                                if let Some(t) = seq.current_pattern_mut().get_track_mut(track_idx)
                                {
                                    if let Some(s) = t.steps.get_mut(idx) {
                                        s.gate_length = gate;
                                    }
                                }
                            }
                        }
                    });
                }
            }
        });
    }
}

/// Paint one row of step cells (`start..end`) and handle interaction.
///
/// Returns `(toggle, extend)` where `toggle` is a step to flip on click and
/// `extend` is `(step, gate_length_in_steps)` produced by dragging a step's
/// gate rightward. Gates that run past the row's right edge are clamped
/// visually to the edge — tails crossing into the next sub-row aren't drawn.
type LaneAction = (Option<usize>, Option<(usize, f32)>);
fn step_lane(
    ui: &mut egui::Ui,
    track_idx: usize,
    start: usize,
    end: usize,
    playhead: usize,
    lane: &[(bool, f32)],
) -> Option<LaneAction> {
    let n = end.saturating_sub(start);
    if n == 0 {
        return None;
    }
    let cell = egui::vec2(18.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(cell.x * n as f32, cell.y),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter();

    // Cells + gate bars.
    for j in 0..n {
        let step_idx = start + j;
        let cell_rect = egui::Rect::from_min_size(
            rect.min + egui::vec2(cell.x * j as f32, 0.0),
            cell - egui::vec2(2.0, 0.0),
        );
        painter.rect_filled(cell_rect, 2.0, egui::Color32::from_gray(40));
        if let Some((active, gate)) = lane.get(step_idx) {
            if *active {
                // Bar spans `gate` steps, clamped to this row's right edge.
                let span = gate.max(0.05).min((end - step_idx) as f32);
                let bar = egui::Rect::from_min_size(
                    cell_rect.min,
                    egui::vec2(cell.x * span - 2.0, cell.y),
                );
                painter.rect_filled(bar, 2.0, egui::Color32::from_rgb(100, 220, 255));
            }
        }
        if step_idx == playhead {
            painter.rect_stroke(
                cell_rect,
                2.0,
                egui::Stroke::new(2.0, egui::Color32::WHITE),
                egui::StrokeKind::Inside,
            );
        }
    }

    // Pointer → step index within this row.
    let cell_at = |pos: egui::Pos2| -> Option<usize> {
        let j = ((pos.x - rect.left()) / cell.x).floor() as i64;
        if j >= 0 && (j as usize) < n {
            Some(start + j as usize)
        } else {
            None
        }
    };

    let drag_id = ui.make_persistent_id(("seq_drag_origin", track_idx, start));
    let mut extend = None;
    if resp.drag_started() {
        if let Some(origin) = resp.interact_pointer_pos().and_then(cell_at) {
            ui.memory_mut(|m| m.data.insert_temp(drag_id, origin));
        }
    }
    if resp.dragged() {
        let origin = ui.memory(|m| m.data.get_temp::<usize>(drag_id));
        if let (Some(origin), Some(cur)) = (
            origin,
            resp.interact_pointer_pos().and_then(cell_at),
        ) {
            let gate = (cur.max(origin) - origin + 1) as f32;
            extend = Some((origin, gate.max(1.0)));
        }
    }

    // A plain click (no drag) toggles the step under the cursor.
    let toggle = if resp.clicked() {
        resp.interact_pointer_pos().and_then(cell_at)
    } else {
        None
    };

    if toggle.is_some() || extend.is_some() {
        Some((toggle, extend))
    } else {
        None
    }
}
