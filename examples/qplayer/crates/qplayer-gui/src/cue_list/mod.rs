//! Cue list — the main view replacing WPF's DataGrid.

use crate::app::{AppCommand, CueType, SharedStateHandle};
use egui::{Color32, RichText};
use qplayer_core::Cue;
use rust_decimal::Decimal;

pub fn show(ui: &mut egui::Ui, state: &SharedStateHandle) {
    let (cues, selected_id, show_mode, active_positions) = {
        let Ok(state) = state.lock() else { return };
        let active_positions: std::collections::HashMap<rust_decimal::Decimal, (usize, Option<usize>)> =
            state.active_cues.iter().map(|ac| (ac.qid, (ac.position, ac.length))).collect();
        (
            state.show_file.cues.clone(),
            state.selected_cue_id,
            state.show_mode,
            active_positions,
        )
    };

    ui.heading(format!("Cues ({})", cues.len()));
    ui.separator();

    // Toolbar
    if show_mode == crate::app::ShowMode::Edit {
        ui.horizontal(|ui| {
            if ui.button("+ Sound").clicked() {
                queue_cmd(state, AppCommand::AddCue { cue_type: CueType::Sound });
            }
            if ui.button("+ Video").clicked() {
                queue_cmd(state, AppCommand::AddCue { cue_type: CueType::Video });
            }
            if ui.button("+ Stop").clicked() {
                queue_cmd(state, AppCommand::AddCue { cue_type: CueType::Stop });
            }
            if ui.button("+ Volume").clicked() {
                queue_cmd(state, AppCommand::AddCue { cue_type: CueType::Volume });
            }
            if ui.button("+ Group").clicked() {
                queue_cmd(state, AppCommand::AddCue { cue_type: CueType::Group });
            }
            if ui.button("+ Dummy").clicked() {
                queue_cmd(state, AppCommand::AddCue { cue_type: CueType::Dummy });
            }
            if ui.button("+ OSC").clicked() {
                queue_cmd(state, AppCommand::AddCue { cue_type: CueType::Osc });
            }
        });
        ui.separator();
    }

    // Header row — use fixed column widths so headers align with body cells.
    const COL_DRAG: f32 = 20.0;
    const COL_QID: f32 = 48.0;
    const COL_NAME: f32 = 140.0;
    const COL_TRIGGER: f32 = 70.0;
    const COL_DURATION: f32 = 60.0;
    const COL_LOOP: f32 = 24.0;
    const COL_TYPE: f32 = 40.0;
    const COL_COLOUR: f32 = 16.0;

    ui.horizontal(|ui| {
        if show_mode == crate::app::ShowMode::Edit {
            ui.add_sized([COL_DRAG, 18.0], egui::Label::new(""));
        }
        ui.add_sized([COL_QID, 18.0], egui::Label::new(RichText::new("#").strong()));
        ui.add_sized([COL_NAME, 18.0], egui::Label::new(RichText::new("Name").strong()));
        ui.add_sized([COL_TRIGGER, 18.0], egui::Label::new(RichText::new("Trigger").strong()));
        ui.add_sized([COL_DURATION, 18.0], egui::Label::new(RichText::new("Duration").strong()));
        ui.add_sized([COL_LOOP, 18.0], egui::Label::new(RichText::new("Loop").strong()));
        ui.add_sized([COL_TYPE, 18.0], egui::Label::new(RichText::new("Type").strong()));
        ui.add_sized([COL_COLOUR, 18.0], egui::Label::new(""));
    });
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        for (idx, cue) in cues.iter().enumerate() {
            let base = cue.base();
            let qid = base.qid;
            let is_selected = selected_id == Some(qid);
            let name = &base.name;
            let cue_type = cue_type_label(cue);
            let colour = colour_to_egui(base.colour);

            let bg = if is_selected {
                ui.visuals().selection.bg_fill
            } else {
                ui.visuals().panel_fill
            };

            let frame = egui::Frame::new()
                .fill(bg)
                .inner_margin(egui::Margin::same(4));

            let (drop_response, dropped_payload) = ui.dnd_drop_zone::<usize, ()>(frame, |ui| {
                ui.horizontal(|ui| {
                    ui.set_min_height(20.0);

                    // Drag handle (only in edit mode)
                    if show_mode == crate::app::ShowMode::Edit {
                        let drag_id = ui.auto_id_with(("drag", idx));
                        ui.dnd_drag_source(drag_id, idx, |ui| {
                            ui.add_sized([COL_DRAG, 18.0], |ui: &mut egui::Ui| {
                                ui.label(egui::RichText::new("≡").monospace().size(14.0))
                            });
                        });
                    }

                    // Q# column
                    if show_mode == crate::app::ShowMode::Edit {
                        let mut qid_str = qid.to_string();
                        let response = ui.add_sized(
                            [COL_QID, 18.0],
                            egui::TextEdit::singleline(&mut qid_str)
                                .id_salt(egui::Id::new(("qid", qid)))
                                .font(egui::TextStyle::Monospace),
                        );
                        if response.lost_focus() {
                            if let Ok(new_qid) = qid_str.parse::<rust_decimal::Decimal>() {
                                if new_qid != qid {
                                    queue_cmd(state, AppCommand::UpdateCueQid { qid, new_qid });
                                }
                            }
                        }
                        if response.clicked() {
                            queue_select(state, qid);
                        }
                    } else {
                        let qid_str = qid.to_string();
                        let response = ui.add_sized([COL_QID, 18.0], |ui: &mut egui::Ui| {
                            ui.selectable_label(is_selected, &qid_str)
                        });
                        if response.clicked() {
                            queue_select(state, qid);
                        }
                    }

                    // Name column
                    if show_mode == crate::app::ShowMode::Edit {
                        let mut name_str = name.clone();
                        let response = ui.add_sized(
                            [COL_NAME, 18.0],
                            egui::TextEdit::singleline(&mut name_str)
                                .id_salt(egui::Id::new(("name", qid)))
                                .font(egui::TextStyle::Body),
                        );
                        if response.changed() && response.lost_focus() {
                            queue_cmd(state, AppCommand::UpdateCueName { qid, name: name_str });
                        }
                        if response.clicked() {
                            queue_select(state, qid);
                        }
                    } else {
                        let response = ui.add_sized([COL_NAME, 18.0], |ui: &mut egui::Ui| {
                            ui.selectable_label(is_selected, name.as_str())
                        });
                        if response.clicked() {
                            queue_select(state, qid);
                        }
                    }

                    // Trigger column — constrain width so the combo doesn't expand the row
                    if show_mode == crate::app::ShowMode::Edit {
                        let mut trigger = base.trigger;
                        ui.add_sized([COL_TRIGGER, 18.0], |ui: &mut egui::Ui| {
                            egui::ComboBox::from_id_salt(egui::Id::new(("trigger", qid)))
                                .selected_text(format!("{:?}", trigger))
                                .width(COL_TRIGGER - 4.0)
                                .show_ui(ui, |ui| {
                                    for mode in [
                                        qplayer_core::TriggerMode::Go,
                                        qplayer_core::TriggerMode::WithLast,
                                        qplayer_core::TriggerMode::AfterLast,
                                    ] {
                                        if ui.selectable_label(trigger == mode, format!("{:?}", mode)).clicked() {
                                            trigger = mode;
                                        }
                                    }
                                })
                                .response
                        });
                        if trigger != base.trigger {
                            queue_cmd(state, AppCommand::UpdateCueTrigger { qid, trigger });
                        }
                    } else {
                        let trigger_label = format!("{:?}", base.trigger);
                        let trigger_short = &trigger_label[..trigger_label.len().min(3)];
                        ui.add_sized([COL_TRIGGER, 18.0], |ui: &mut egui::Ui| {
                            ui.label(RichText::new(trigger_short).monospace().size(10.0))
                        });
                    }

                    // Duration / Progress column
                    let duration_str = match cue {
                        qplayer_core::Cue::Sound { duration, .. }
                        | qplayer_core::Cue::Video { duration, .. }
                        | qplayer_core::Cue::TimeCode { duration, .. } => {
                            if duration.as_secs_f64() > 0.0 {
                                format_duration(duration)
                            } else {
                                "—".to_string()
                            }
                        }
                        _ => "—".to_string(),
                    };
                    ui.add_sized([COL_DURATION, 18.0], |ui: &mut egui::Ui| {
                        if let Some((pos, len)) = active_positions.get(&qid) {
                            if let Some(len) = len {
                                if *len > 0 {
                                    let progress = (*pos as f32 / *len as f32).clamp(0.0, 1.0);
                                    let bar_width = COL_DURATION - 4.0;
                                    let bar_height = 6.0;
                                    let (rect, _response) = ui.allocate_exact_size(
                                        egui::vec2(bar_width, bar_height),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().rect_filled(rect, 2.0, Color32::from_rgb(40, 40, 40));
                                    let fill_rect = egui::Rect::from_min_size(
                                        rect.min,
                                        egui::vec2(bar_width * progress, bar_height),
                                    );
                                    ui.painter().rect_filled(fill_rect, 2.0, Color32::from_rgb(100, 180, 100));
                                    return _response;
                                }
                            }
                        }
                        ui.label(RichText::new(&duration_str).monospace().size(10.0))
                    });

                    // Loop column
                    let loop_short = match base.loop_mode {
                        qplayer_core::LoopMode::OneShot => "1",
                        qplayer_core::LoopMode::Looped => &format!("{}", base.loop_count),
                        qplayer_core::LoopMode::LoopedInfinite => "∞",
                        qplayer_core::LoopMode::HoldLast => "H",
                    };
                    ui.add_sized([COL_LOOP, 18.0], |ui: &mut egui::Ui| {
                        ui.label(RichText::new(loop_short).monospace().size(10.0))
                    });

                    // Type column
                    ui.add_sized([COL_TYPE, 18.0], |ui: &mut egui::Ui| {
                        ui.label(RichText::new(cue_type).monospace().size(10.0))
                    });

                    // Colour swatch
                    ui.add_sized([COL_COLOUR, 18.0], |ui: &mut egui::Ui| {
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(COL_COLOUR, 16.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(rect, 4.0, colour);
                        response
                    });
                });
            });

            // Context menu on the entire row (right-click anywhere in the frame)
            if show_mode == crate::app::ShowMode::Edit {
                drop_response.response.context_menu(|ui| {
                    if ui.button("Move Up").clicked() {
                        queue_cmd(state, AppCommand::MoveSelectedCueUp);
                        ui.close();
                    }
                    if ui.button("Move Down").clicked() {
                        queue_cmd(state, AppCommand::MoveSelectedCueDown);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Duplicate").clicked() {
                        queue_cmd(state, AppCommand::DuplicateSelectedCue);
                        ui.close();
                    }
                    if ui.button("Delete").clicked() {
                        queue_cmd(state, AppCommand::DeleteSelectedCue);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Add Sound Cue").clicked() {
                        queue_cmd(state, AppCommand::AddCue { cue_type: CueType::Sound });
                        ui.close();
                    }
                    if ui.button("Add Video Cue").clicked() {
                        queue_cmd(state, AppCommand::AddCue { cue_type: CueType::Video });
                        ui.close();
                    }
                    if ui.button("Add Stop Cue").clicked() {
                        queue_cmd(state, AppCommand::AddCue { cue_type: CueType::Stop });
                        ui.close();
                    }
                    if ui.button("Add Volume Cue").clicked() {
                        queue_cmd(state, AppCommand::AddCue { cue_type: CueType::Volume });
                        ui.close();
                    }
                });
            }

            // Handle dropped payload for reordering
            if show_mode == crate::app::ShowMode::Edit {
                if let Some(source_idx) = dropped_payload {
                    let source = *source_idx;
                    if source != idx {
                        queue_cmd(state, AppCommand::MoveCue { from_idx: source, to_idx: idx });
                    }
                }
            }
        }
    });

    if show_mode == crate::app::ShowMode::Show {
        ui.horizontal(|ui| {
            ui.colored_label(Color32::YELLOW, "● SHOW MODE");
            ui.label("Editing disabled");
        });
    }
}

fn queue_select(state: &SharedStateHandle, qid: Decimal) {
    if let Ok(mut state) = state.lock() {
        state.command_queue.push(AppCommand::SelectCue(qid));
    }
}

fn queue_cmd(state: &SharedStateHandle, cmd: AppCommand) {
    if let Ok(mut state) = state.lock() {
        state.command_queue.push(cmd);
    }
}

fn cue_type_label(cue: &Cue) -> &'static str {
    match cue {
        Cue::Group { .. } => "GRP",
        Cue::Sound { .. } => "SND",
        Cue::Video { .. } => "VID",
        Cue::Stop { .. } => "STP",
        Cue::Volume { .. } => "VOL",
        Cue::Dummy { .. } => "DUM",
        Cue::TimeCode { .. } => "TC",
        Cue::Osc { .. } => "OSC",
    }
}

fn colour_to_egui(c: qplayer_core::SerializedColour) -> Color32 {
    Color32::from_rgba_premultiplied(
        (c.r * 255.0) as u8,
        (c.g * 255.0) as u8,
        (c.b * 255.0) as u8,
        (c.a * 255.0) as u8,
    )
}

fn format_duration(d: &qplayer_core::Timespan) -> String {
    let secs = d.as_secs_f64();
    let mins = (secs / 60.0) as u64;
    let rem_secs = secs % 60.0;
    if mins > 0 {
        format!("{}:{:05.2}", mins, rem_secs)
    } else {
        format!("{:.2}s", secs)
    }
}
