//! Presets tab — quick slots, save/load/delete, assign to slot.

use crate::egui_control_gui::EguiControlGui;
use crate::egui_theme::colors::*;
use egui::Color32;
use rustjay_core::PresetCommand;

impl EguiControlGui {
    pub(crate) fn build_presets_tab(&mut self, ui: &mut egui::Ui) {
        let (preset_names, slot_names) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.preset_names.clone(), state.preset_quick_slot_names.clone())
        };

        // ── Quick Slots ──────────────────────────────────────────────────────
        ui.heading("Quick Slots");
        ui.add_space(8.0);

        let avail = ui.available_width();
        let gap = 6.0;
        let btn_w = ((avail - gap * 3.0) / 4.0).max(50.0);

        for slot in 1..=8usize {
            let has_preset = slot_names[slot - 1].is_some();
            let color = if has_preset { ACCENT_CYAN } else { BG_WIDGET };
            let text_color = if has_preset { Color32::BLACK } else { TEXT_SECONDARY };
            let label = if let Some(ref name) = slot_names[slot - 1] {
                let short: String = name.chars().take(7).collect();
                format!("{}\n{}", slot, short)
            } else {
                format!("{}\n--", slot)
            };

            let btn = egui::Button::new(egui::RichText::new(&label).color(text_color).size(12.0))
                .fill(color)
                .min_size(egui::vec2(btn_w, 50.0));

            let response = ui.add_sized(egui::vec2(btn_w, 50.0), btn);
            if response.clicked() && has_preset {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.preset_command = PresetCommand::ApplySlot(slot);
            }
            if response.hovered() {
                if let Some(ref name) = slot_names[slot - 1] {
                    response.on_hover_text(format!("Slot {}: {} (click to apply)", slot, name));
                } else {
                    response.on_hover_text(format!("Slot {} — right-click a preset below to assign", slot));
                }
            }

            if slot % 4 != 0 && slot < 8 {
                // slots flow left-to-right naturally; no same_line needed in egui
            }
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        // ── Preset Management ────────────────────────────────────────────────
        if !self.saving_preset {
            ui.horizontal(|ui| {
                if ui.button("💾 Save New Preset").clicked() {
                    self.saving_preset = true;
                }
                if ui.button("🔄 Refresh List").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.preset_command = PresetCommand::Refresh;
                }
            });
        } else {
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.text_edit_singleline(&mut self.preset_name_buffer);
                if ui.button("Save").clicked() && !self.preset_name_buffer.is_empty() {
                    let name = self.preset_name_buffer.clone();
                    self.preset_name_buffer.clear();
                    self.saving_preset = false;
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.preset_command = PresetCommand::Save { name };
                }
                if ui.button("Cancel").clicked() {
                    self.preset_name_buffer.clear();
                    self.saving_preset = false;
                }
            });
        }

        ui.add_space(8.0);

        // ── Preset List ──────────────────────────────────────────────────────
        if preset_names.is_empty() {
            ui.label(egui::RichText::new("No presets saved yet.").color(TEXT_SECONDARY));
        } else {
            ui.label(format!("{} preset(s) — click to load, right-click to assign to slot", preset_names.len()));
        }

        ui.add_space(4.0);
        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
            for (index, name) in preset_names.iter().enumerate() {
                let response = ui.selectable_label(false, name);
                if response.clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.preset_command = PresetCommand::Load(index);
                }

                // Right-click context menu
                response.context_menu(|ui| {
                    ui.label(egui::RichText::new(name).strong().color(TEXT_SECONDARY));
                    ui.separator();
                    if ui.button("Load").clicked() {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.preset_command = PresetCommand::Load(index);
                        ui.close();
                    }
                    ui.separator();
                    ui.label("Assign to slot:");
                    for slot in 1..=8usize {
                        let slot_label = if let Some(ref sname) = slot_names[slot - 1] {
                            format!("Slot {} ({})", slot, sname)
                        } else {
                            format!("Slot {} — empty", slot)
                        };
                        if ui.button(&slot_label).clicked() {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.preset_command = PresetCommand::AssignSlot {
                                preset_index: index,
                                slot,
                            };
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("Delete").clicked() {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.preset_command = PresetCommand::Delete(index);
                        ui.close();
                    }
                });
            }
        });
    }
}
