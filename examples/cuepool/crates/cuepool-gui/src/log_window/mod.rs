//! In-app log viewer window.

use crate::app::SharedStateHandle;
use crate::logging::{clear_log_buffer, read_log_buffer};

pub fn show(ui: &mut egui::Ui, _state: &SharedStateHandle) {
    let entries = read_log_buffer();

    // Toolbar
    ui.horizontal(|ui| {
        if ui.button("Clear").clicked() {
            clear_log_buffer();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(format!("{} entries", entries.len()));
        });
    });
    ui.separator();

    // Log table
    let text_style = egui::TextStyle::Monospace;
    let row_height = ui.text_style_height(&text_style) + 2.0;

    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .stick_to_bottom(true)
        .show_rows(ui, row_height, entries.len(), |ui, row_range| {
            for i in row_range {
                let entry = &entries[i];
                let (color, level_str) = match entry.level {
                    log::Level::Error => (egui::Color32::from_rgb(255, 80, 80), "ERR "),
                    log::Level::Warn => (egui::Color32::from_rgb(255, 200, 80), "WARN"),
                    log::Level::Info => (egui::Color32::from_rgb(180, 220, 255), "INFO"),
                    log::Level::Debug => (egui::Color32::from_rgb(160, 160, 160), "DBG "),
                    log::Level::Trace => (egui::Color32::from_rgb(120, 120, 120), "TRC "),
                };

                ui.horizontal(|ui| {
                    ui.monospace(
                        egui::RichText::new(level_str)
                            .color(color)
                            .monospace(),
                    );
                    ui.monospace(
                        egui::RichText::new(&entry.timestamp)
                            .color(egui::Color32::from_rgb(140, 140, 140))
                            .monospace(),
                    );
                    ui.monospace(
                        egui::RichText::new(&entry.message)
                            .color(egui::Color32::LIGHT_GRAY)
                            .monospace(),
                    );
                });
            }
        });
}
