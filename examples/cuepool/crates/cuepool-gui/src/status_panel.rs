use crate::app::SharedStateHandle;

/// Status window body: live engine/render-loop diagnostics a designer can copy
/// into a bug report. Renders the same `Diagnostics::sections()` rows that the
/// clipboard dump uses, so the two can't drift.
pub fn show(
    ui: &mut egui::Ui,
    state: &SharedStateHandle,
    copied_at: &mut Option<std::time::Instant>,
) {
    let diagnostics = state
        .lock()
        .map(|s| s.diagnostics.clone())
        .unwrap_or_default();

    ui.horizontal(|ui| {
        if ui.button("Copy to Clipboard").clicked() {
            ui.ctx().copy_text(diagnostics.to_text());
            *copied_at = Some(std::time::Instant::now());
        }
        if copied_at.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(2)) {
            ui.label("Copied!");
        }
    });
    ui.separator();

    for (title, rows) in diagnostics.sections() {
        egui::CollapsingHeader::new(title)
            .default_open(true)
            .show(ui, |ui| {
                egui::Grid::new(format!("status_grid_{title}"))
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (key, value) in rows {
                            ui.monospace(key);
                            ui.monospace(value);
                            ui.end_row();
                        }
                    });
            });
    }
}
