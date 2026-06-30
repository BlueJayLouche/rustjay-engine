//! Projection mapping configuration panel.

use crate::app::{AppCommand, SharedStateHandle};
use egui::RichText;

pub fn show(ui: &mut egui::Ui, state: &SharedStateHandle) {
    let Ok(mut state) = state.lock() else { return };

    // ponytail: whole-state clone per panel frame so every edit is undoable.
    let pre_edit = crate::app::Snapshot::from_state(&state).with_merge_key("projection");
    let mut changed = false;
    // Clone the monitor list before borrowing `projection` (the control binary
    // publishes it into shared state each frame) so the dropdown can list real
    // monitors instead of a blind index.
    let available_monitors = state.available_monitors.clone();

    let projection = &mut state.show_file.projection;

    ui.heading("Projection Canvas");
    ui.separator();

    ui.horizontal(|ui| {
        ui.label("Canvas Width:");
        let mut w = projection.canvas_width as i32;
        if ui.add(egui::DragValue::new(&mut w).speed(1).range(1..=16384)).changed() {
            projection.canvas_width = w.max(1) as u32;
            changed = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("Canvas Height:");
        let mut h = projection.canvas_height as i32;
        if ui.add(egui::DragValue::new(&mut h).speed(1).range(1..=16384)).changed() {
            projection.canvas_height = h.max(1) as u32;
            changed = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("Video Fit:");
        egui::ComboBox::from_id_salt("projection_fit")
            .selected_text(format!("{:?}", projection.fit))
            .show_ui(ui, |ui| {
                for variant in [
                    qplayer_core::CanvasFit::Fit,
                    qplayer_core::CanvasFit::Fill,
                    qplayer_core::CanvasFit::Stretch,
                ] {
                    if ui.selectable_value(&mut projection.fit, variant, format!("{:?}", variant)).clicked() {
                        changed = true;
                    }
                }
            });
    });

    ui.separator();

    ui.horizontal(|ui| {
        ui.heading("Outputs");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("3×1 Edge Blend Preset").clicked() {
                *projection = qplayer_core::ProjectionConfig::preset_3x1_edgeblend();
                changed = true;
            }
            if ui.button("Default Single Output").clicked() {
                *projection = qplayer_core::ProjectionConfig::default_single();
                changed = true;
            }
        });
    });

    let mut remove_idx: Option<usize> = None;
    let mut duplicate_idx: Option<usize> = None;

    for (idx, output) in projection.outputs.iter_mut().enumerate() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Output {}", idx + 1)).strong());
                if ui.small_button("✕").clicked() {
                    remove_idx = Some(idx);
                }
                if ui.small_button("⎘").clicked() {
                    duplicate_idx = Some(idx);
                }
            });

            ui.horizontal(|ui| {
                ui.label("Name:");
                changed |= ui.text_edit_singleline(&mut output.name).changed();
            });

            ui.horizontal(|ui| {
                ui.label("Source X:");
                let mut v = output.source_x as i32;
                if ui.add(egui::DragValue::new(&mut v).speed(1).range(0..=16384)).changed() {
                    output.source_x = v.max(0) as u32;
                    changed = true;
                }
                ui.label("Y:");
                let mut v = output.source_y as i32;
                if ui.add(egui::DragValue::new(&mut v).speed(1).range(0..=16384)).changed() {
                    output.source_y = v.max(0) as u32;
                    changed = true;
                }
                ui.label("W:");
                let mut v = output.source_width as i32;
                if ui.add(egui::DragValue::new(&mut v).speed(1).range(1..=16384)).changed() {
                    output.source_width = v.max(1) as u32;
                    changed = true;
                }
                ui.label("H:");
                let mut v = output.source_height as i32;
                if ui.add(egui::DragValue::new(&mut v).speed(1).range(1..=16384)).changed() {
                    output.source_height = v.max(1) as u32;
                    changed = true;
                }
            });

            ui.horizontal(|ui| {
                ui.label("Output W:");
                let mut v = output.output_width as i32;
                if ui.add(egui::DragValue::new(&mut v).speed(1).range(1..=16384)).changed() {
                    output.output_width = v.max(1) as u32;
                    changed = true;
                }
                ui.label("H:");
                let mut v = output.output_height as i32;
                if ui.add(egui::DragValue::new(&mut v).speed(1).range(1..=16384)).changed() {
                    output.output_height = v.max(1) as u32;
                    changed = true;
                }
            });

            ui.horizontal(|ui| {
                ui.label("Fullscreen Monitor:");
                let current = output
                    .monitor_id
                    .as_ref()
                    .map(|m| m.label())
                    .unwrap_or_else(|| "Windowed".to_string());
                egui::ComboBox::from_id_salt(("fs_monitor", idx))
                    .selected_text(current)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(output.monitor_id.is_none(), "Windowed").clicked() {
                            output.monitor_id = None;
                            output.fullscreen_monitor = None;
                            changed = true;
                        }
                        for mon in &available_monitors {
                            let selected = output.monitor_id.as_ref() == Some(mon);
                            if ui.selectable_label(selected, mon.label()).clicked() {
                                // Descriptor (recall by position) supersedes the index.
                                output.monitor_id = Some(mon.clone());
                                output.fullscreen_monitor = None;
                                changed = true;
                            }
                        }
                    })
                    .response
                    .on_hover_text("Recalled by monitor position, so it survives reboots / projector reorder.");
            });
            if available_monitors.is_empty() {
                ui.label(RichText::new("(no monitors reported yet)").weak().size(10.0));
            }

            ui.separator();
            ui.label(RichText::new("Edge Blend").strong().size(11.0));
            changed |= edge_editor(ui, "Left", &mut output.edge_blend.left);
            changed |= edge_editor(ui, "Right", &mut output.edge_blend.right);
            changed |= edge_editor(ui, "Top", &mut output.edge_blend.top);
            changed |= edge_editor(ui, "Bottom", &mut output.edge_blend.bottom);
        });
    }

    if ui.button("+ Add Output").clicked() {
        let mut output = qplayer_core::ProjectorOutput::default_single();
        output.name = format!("Output {}", projection.outputs.len() + 1);
        projection.outputs.push(output);
        changed = true;
    }

    if let Some(idx) = remove_idx {
        if projection.outputs.len() > 1 {
            projection.outputs.remove(idx);
            changed = true;
        }
    }

    if let Some(idx) = duplicate_idx {
        if let Some(original) = projection.outputs.get(idx).cloned() {
            let mut copy = original;
            copy.name.push_str(" (copy)");
            projection.outputs.insert(idx + 1, copy);
            changed = true;
        }
    }

    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Open Projection Output Windows").clicked() {
            state.command_queue.push(AppCommand::OpenProjectionOutputs);
        }
        if ui
            .button("Identify Outputs")
            .on_hover_text("Flash each output a distinct colour so you can see which window is on which projector.")
            .clicked()
        {
            state.identify_outputs = true;
        }
    });

    if changed {
        state.dirty = true;
        state.undo_redo.push(pre_edit);
    }
}

fn edge_editor(ui: &mut egui::Ui, label: &str, edge: &mut qplayer_core::EdgeBlendEdge) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut enabled = edge.enabled;
        if ui.checkbox(&mut enabled, label).changed() {
            edge.enabled = enabled;
            changed = true;
        }
        ui.add_space(8.0);
        ui.label("Width (px):");
        let mut w = edge.width as i32;
        if ui.add(egui::DragValue::new(&mut w).speed(1).range(0..=4096)).changed() {
            edge.width = w.max(0) as u32;
            changed = true;
        }
        ui.label("Gamma:");
        let mut g = edge.gamma;
        if ui.add(egui::DragValue::new(&mut g).speed(0.05).range(0.1..=5.0)).changed() {
            edge.gamma = g;
            changed = true;
        }
    });
    changed
}
