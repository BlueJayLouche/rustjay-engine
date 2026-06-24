//! Projection mapping configuration panel.

use crate::app::{AppCommand, SharedStateHandle};
use egui::RichText;

pub fn show(ui: &mut egui::Ui, state: &SharedStateHandle) {
    let Ok(mut state) = state.lock() else { return };

    // ponytail: whole-state clone per panel frame so every edit is undoable.
    let pre_edit = crate::app::Snapshot::from_state(&state).with_merge_key("projection");
    let mut changed = false;

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
                let mut mon = output.fullscreen_monitor.map(|m| m as i32).unwrap_or(-1);
                if ui.add(egui::DragValue::new(&mut mon).speed(1).range(-1..=15)).changed() {
                    output.fullscreen_monitor = if mon < 0 { None } else { Some(mon as usize) };
                    changed = true;
                }
                ui.label("(-1 = windowed)").on_hover_text("Monitor index for borderless fullscreen. 0 = primary.");
            });

            ui.separator();
            ui.label(RichText::new("Edge Blend").strong().size(11.0));
            changed |= edge_editor(ui, "Left", &mut output.edge_blend.left);
            changed |= edge_editor(ui, "Right", &mut output.edge_blend.right);
            changed |= edge_editor(ui, "Top", &mut output.edge_blend.top);
            changed |= edge_editor(ui, "Bottom", &mut output.edge_blend.bottom);
        });
    }

    if ui.button("+ Add Output").clicked() {
        projection.outputs.push(qplayer_core::ProjectorOutput::default_single());
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
    if ui.button("Open Projection Output Windows").clicked() {
        state.command_queue.push(AppCommand::OpenProjectionOutputs);
    }

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
