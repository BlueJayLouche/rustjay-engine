use cuepool_gui::{CuePoolApp, preview};
use eframe::egui;

struct PreviewApp {
    app: CuePoolApp,
}

impl eframe::App for PreviewApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| self.app.update(ui));

        if let Ok(mut state) = self.app.state().lock() {
            for command in state.command_queue.drain(..) {
                eprintln!("{command:?}");
            }
        }
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "CuePool UI Preview",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_theme(egui::Theme::Dark);
            cc.egui_ctx.global_style_mut(|style| {
                style.interaction.selectable_labels = false;
                style.interaction.multi_widget_text_select = false;
            });
            Ok(Box::new(PreviewApp {
                app: preview::demo_app(),
            }))
        }),
    )
}
