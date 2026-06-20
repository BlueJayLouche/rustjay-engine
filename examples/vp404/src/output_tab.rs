//! Output senders tab: NDI / Syphon / Spout / V4L2 / disk recording.
//!
//! Sets `engine.output_command` each frame to start/stop senders; the engine
//! processes the command and updates the `*_output.enabled / is_active` state
//! fields read back here for status display.

use rustjay_core::OutputCommand;
use rustjay_engine::prelude::*;

pub struct OutputTab {
    /// Editable sender name used for NDI / Syphon / Spout.
    sender_name: String,
    /// Recording output path (defaults to current dir).
    record_path: String,
    record_codec: rustjay_core::RecorderCodec,
}

impl OutputTab {
    pub fn new(app_name: &str) -> Self {
        Self {
            sender_name: app_name.to_string(),
            record_path: format!("./{app_name}.mov"),
            record_codec: rustjay_core::RecorderCodec::ProRes422,
        }
    }
}

impl AnyEguiTab for OutputTab {
    fn name(&self) -> &str {
        "Output"
    }

    fn replaces(&self) -> Option<BuiltinTab> {
        Some(BuiltinTab::Output)
    }

    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        _app_state: &mut dyn std::any::Any,
        engine: &mut EngineState,
    ) {
        ui.heading("Output Senders");
        ui.separator();

        // Sender name (shared by NDI / Syphon / Spout)
        ui.horizontal(|ui| {
            ui.label("Sender name:");
            ui.text_edit_singleline(&mut self.sender_name);
        });
        ui.add_space(8.0);

        // --- Syphon (macOS) --------------------------------------------------
        #[cfg(target_os = "macos")]
        {
            let active = engine.syphon_output.enabled;
            ui.horizontal(|ui| {
                let label = if active { "⏹ Stop Syphon" } else { "▶ Syphon" };
                if ui.button(label).clicked() {
                    if active {
                        engine.output_command = OutputCommand::StopSyphon;
                    } else {
                        engine.syphon_output.server_name = self.sender_name.clone();
                        engine.output_command = OutputCommand::StartSyphon;
                    }
                }
                if active {
                    ui.label(egui::RichText::new("● LIVE").color(egui::Color32::GREEN).strong());
                }
            });
            ui.add_space(4.0);
        }

        // --- NDI -------------------------------------------------------------
        #[cfg(feature = "ndi")]
        {
            let active = engine.ndi_output.is_active;
            ui.horizontal(|ui| {
                let label = if active { "⏹ Stop NDI" } else { "▶ NDI" };
                if ui.button(label).clicked() {
                    if active {
                        engine.output_command = OutputCommand::StopNdi;
                    } else {
                        engine.ndi_output.stream_name = self.sender_name.clone();
                        engine.output_command = OutputCommand::StartNdi;
                    }
                }
                if active {
                    ui.label(egui::RichText::new("● LIVE").color(egui::Color32::GREEN).strong());
                }
            });
            ui.add_space(4.0);
        }

        // --- Spout (Windows) -------------------------------------------------
        #[cfg(target_os = "windows")]
        {
            let active = engine.spout_output.enabled;
            ui.horizontal(|ui| {
                let label = if active { "⏹ Stop Spout" } else { "▶ Spout" };
                if ui.button(label).clicked() {
                    if active {
                        engine.output_command = OutputCommand::StopSpout;
                    } else {
                        engine.output_command = OutputCommand::StartSpout {
                            sender_name: self.sender_name.clone(),
                        };
                    }
                }
                if active {
                    ui.label(egui::RichText::new("● LIVE").color(egui::Color32::GREEN).strong());
                }
            });
            ui.add_space(4.0);
        }

        // --- V4L2 (Linux) ----------------------------------------------------
        #[cfg(target_os = "linux")]
        {
            let active = engine.v4l2_output.enabled;
            ui.horizontal(|ui| {
                ui.label("Device:");
                ui.text_edit_singleline(&mut engine.v4l2_output.device_path);
            });
            ui.horizontal(|ui| {
                let label = if active { "⏹ Stop V4L2" } else { "▶ V4L2" };
                if ui.button(label).clicked() {
                    if active {
                        engine.output_command = OutputCommand::StopV4l2;
                    } else {
                        engine.output_command = OutputCommand::StartV4l2 {
                            device_path: engine.v4l2_output.device_path.clone(),
                        };
                    }
                }
                if active {
                    ui.label(egui::RichText::new("● LIVE").color(egui::Color32::GREEN).strong());
                }
            });
            ui.add_space(4.0);
        }

        ui.separator();

        // --- Recording -------------------------------------------------------
        ui.label(egui::RichText::new("Recording").strong());
        let recording = engine.recording_active;
        if !recording {
            ui.horizontal(|ui| {
                ui.label("Path:");
                ui.text_edit_singleline(&mut self.record_path);
                if ui.button("…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Video", &["mp4", "mov"])
                        .save_file()
                    {
                        self.record_path = path.display().to_string();
                    }
                }
            });
            egui::ComboBox::from_label("Codec")
                .selected_text(codec_label(self.record_codec))
                .show_ui(ui, |ui| {
                    use rustjay_core::RecorderCodec;
                    for c in [RecorderCodec::ProRes422, RecorderCodec::H264, RecorderCodec::H265, RecorderCodec::AV1] {
                        ui.selectable_value(&mut self.record_codec, c, codec_label(c));
                    }
                });
            if ui.button("⏺ Record").clicked() {
                engine.output_command = OutputCommand::StartRecording {
                    path: self.record_path.clone(),
                    codec: self.record_codec,
                };
            }
        } else {
            ui.horizontal(|ui| {
                if ui.button("⏹ Stop recording").clicked() {
                    engine.output_command = OutputCommand::StopRecording;
                }
                ui.label(egui::RichText::new("● REC").color(egui::Color32::RED).strong());
            });
        }
    }
}

fn codec_label(c: rustjay_core::RecorderCodec) -> &'static str {
    use rustjay_core::RecorderCodec;
    match c {
        RecorderCodec::H264 => "H.264",
        RecorderCodec::H265 => "H.265 / HEVC",
        RecorderCodec::AV1 => "AV1",
        RecorderCodec::ProRes422 => "ProRes 422",
    }
}
