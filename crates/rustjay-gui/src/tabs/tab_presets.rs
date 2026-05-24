use crate::control_gui::ControlGui;
use rustjay_core::PresetCommand;

impl ControlGui {
    /// Build the Presets tab
    pub(crate) fn build_presets_tab(&mut self, ui: &imgui::Ui) {
        let (preset_names, slot_names) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.preset_names.clone(), state.preset_quick_slot_names.clone())
        };

        // ── Quick Slots ──────────────────────────────────────────────────────
        ui.text("Quick Slots");
        ui.separator();

        // Compute a button width that tiles 4-per-row with 6px gaps.
        let avail = ui.content_region_avail()[0];
        let btn_w = ((avail - 18.0) / 4.0).max(50.0); // 3 gaps × 6px = 18px
        let btn_size = [btn_w, 50.0];

        for slot in 1..=8usize {
            let has_preset = slot_names[slot - 1].is_some();
            let color = if has_preset { [0.2, 0.6, 1.0, 1.0] } else { [0.3, 0.3, 0.3, 1.0] };
            let label = if let Some(ref name) = slot_names[slot - 1] {
                let short: String = name.chars().take(7).collect();
                format!("{}\n{}", slot, short)
            } else {
                format!("{}\n--", slot)
            };

            let _col = ui.push_style_color(imgui::StyleColor::Button, color);
            if ui.button_with_size(&label, btn_size) && has_preset {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.preset_command = PresetCommand::ApplySlot(slot);
            }

            if ui.is_item_hovered() {
                if let Some(ref name) = slot_names[slot - 1] {
                    ui.tooltip_text(format!("Slot {}: {} (click to apply)", slot, name));
                } else {
                    ui.tooltip_text(format!("Slot {} — right-click a preset below to assign", slot));
                }
            }

            if slot % 4 != 0 {
                ui.same_line_with_spacing(0.0, 6.0);
            }
        }

        ui.spacing();
        ui.separator();
        ui.spacing();

        // ── Preset Management ─────────────────────────────────────────────────
        if !self.saving_preset {
            if ui.button("Save New Preset") {
                self.saving_preset = true;
            }
            ui.same_line();
            if ui.button("Refresh List") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.preset_command = PresetCommand::Refresh;
            }
        } else {
            ui.text("Name:");
            ui.same_line();
            let _w = ui.push_item_width(-120.0);
            ui.input_text("##preset_name", &mut self.preset_name_buffer)
                .build();
            drop(_w);
            ui.same_line();
            if ui.button("Save") && !self.preset_name_buffer.is_empty() {
                let name = self.preset_name_buffer.clone();
                self.preset_name_buffer.clear();
                self.saving_preset = false;
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.preset_command = PresetCommand::Save { name };
            }
            ui.same_line();
            if ui.button("Cancel") {
                self.preset_name_buffer.clear();
                self.saving_preset = false;
            }
        }

        ui.spacing();

        // ── Preset List ───────────────────────────────────────────────────────
        if preset_names.is_empty() {
            ui.text_disabled("No presets saved yet.");
        } else {
            ui.text(format!("{} preset(s)  —  click to load, right-click to assign to slot", preset_names.len()));
        }

        ui.child_window("presets_list")
            .size([0.0, 0.0])
            .build(|| {
                for (index, name) in preset_names.iter().enumerate() {
                    let _col = ui.push_style_color(
                        imgui::StyleColor::Header,
                        [0.2, 0.4, 0.7, 1.0],
                    );
                    let selected = false;
                    if ui.selectable_config(name).selected(selected).build() {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.preset_command = PresetCommand::Load(index);
                    }
                    drop(_col);

                    // Right-click context menu: assign to slot or delete
                    if ui.is_item_hovered() && ui.is_mouse_clicked(imgui::MouseButton::Right) {
                        ui.open_popup(format!("##preset_ctx_{}", index));
                    }

                    if let Some(_popup) = ui.begin_popup(format!("##preset_ctx_{}", index)) {
                        ui.text_disabled(name);
                        ui.separator();
                        if ui.menu_item("Load") {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.preset_command = PresetCommand::Load(index);
                        }
                        ui.separator();
                        ui.text_disabled("Assign to slot:");
                        for slot in 1..=8usize {
                            let slot_label = if let Some(ref sname) = slot_names[slot - 1] {
                                format!("Slot {} ({})", slot, sname)
                            } else {
                                format!("Slot {} — empty", slot)
                            };
                            if ui.menu_item(&slot_label) {
                                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                state.preset_command = PresetCommand::AssignSlot {
                                    preset_index: index,
                                    slot,
                                };
                            }
                        }
                        ui.separator();
                        if ui.menu_item("Delete") {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.preset_command = PresetCommand::Delete(index);
                        }
                    }
                }
            });
    }
}
