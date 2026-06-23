//! Transport controls — Go, Stop, Pause buttons.

use crate::app::{AppCommand, GuiMeterData, SharedStateHandle};
use egui::{Button, Color32, RichText, Vec2};

pub fn show(ui: &mut egui::Ui, state: &SharedStateHandle) {
    ui.horizontal(|ui| {
        let button_size = Vec2::new(60.0, 32.0);

        let go_btn = Button::new(RichText::new("▶ GO").strong().color(Color32::WHITE))
            .fill(Color32::from_rgb(0, 180, 0))
            .min_size(button_size);
        if ui.add(go_btn).clicked() {
            if let Ok(mut state) = state.lock() {
                state.command_queue.push(AppCommand::Go);
            }
        }

        let stop_btn = Button::new(RichText::new("⏹ STOP").strong())
            .fill(Color32::from_rgb(200, 0, 0))
            .min_size(button_size);
        if ui.add(stop_btn).clicked() {
            if let Ok(mut state) = state.lock() {
                state.command_queue.push(AppCommand::Stop);
            }
        }

        let pause_btn = Button::new(RichText::new("⏸ PAUSE"))
            .min_size(button_size);
        if ui.add(pause_btn).clicked() {
            if let Ok(mut state) = state.lock() {
                state.command_queue.push(AppCommand::Pause);
            }
        }

        let preload_btn = Button::new(RichText::new("PRELOAD"))
            .min_size(Vec2::new(70.0, 32.0));
        if ui.add(preload_btn).clicked() {
            if let Ok(mut state) = state.lock() {
                state.command_queue.push(AppCommand::Preload);
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
        if ui.add(mode_btn).clicked() {
            if let Ok(mut state) = state.lock() {
                let snapshot = crate::app::Snapshot::from_state(&state);
                state.undo_redo.push(snapshot);
                state.show_mode = match state.show_mode {
                    crate::app::ShowMode::Edit => crate::app::ShowMode::Show,
                    crate::app::ShowMode::Show => crate::app::ShowMode::Edit,
                };
                state.dirty = true;
            }
        }

        // Master meter bridge
        let meter_data = {
            let Ok(state) = state.lock() else { return };
            state.meter_data
        };
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            draw_meter(ui, &meter_data);
        });
    });
}

fn draw_meter(ui: &mut egui::Ui, data: &GuiMeterData) {
    let width = 8.0;
    let height = 32.0;
    let _gap = 4.0;

    for &(peak_db, rms_db) in &[(data.peak_l_db, data.rms_l_db), (data.peak_r_db, data.rms_r_db)] {
        let (rect, _response) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
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
