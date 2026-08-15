//! DMX Recorder panel — record incoming Art-Net/sACN into `.dmxrec` takes.
//!
//! The GUI owns the target file path; recording state lives engine-side and
//! is mirrored back via [`RecorderStatus`](crate::app::RecorderStatus).

use crate::app::{AppCommand, SharedStateHandle};

pub fn show(ui: &mut egui::Ui, state: &SharedStateHandle) {
    let Ok(mut state) = state.lock() else { return };
    let status = state.recorder_status.clone();
    let mut cmds: Vec<AppCommand> = Vec::new();

    ui.add_enabled_ui(!status.recording, |ui| {
        ui.horizontal(|ui| {
            ui.label("Take:");
            ui.text_edit_singleline(&mut state.recorder_file);
            if ui.button("Browse…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("DMX recording", &["dmxrec"])
                    .pick_file()
            {
                state.recorder_file = path.to_string_lossy().to_string();
            }
            if ui.button("New…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("DMX recording", &["dmxrec"])
                    .set_file_name("take.dmxrec")
                    .save_file()
            {
                state.recorder_file = path.to_string_lossy().to_string();
            }
        });
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let record_label = if status.recording { "⏹ Stop & Keep" } else { "⏺ Record" };
        if ui
            .add_enabled(!state.recorder_file.trim().is_empty(), egui::Button::new(record_label))
            .on_hover_text(
                "Record incoming sACN/Art-Net. Over an existing take, a channel \
                 punches in when its value first deviates and stays live until the pass ends.",
            )
            .clicked()
        {
            cmds.push(AppCommand::RecorderRecord { file: state.recorder_file.clone() });
        }
        if ui
            .add_enabled(status.recording, egui::Button::new("Discard"))
            .on_hover_text("Stop and throw the pass away — the take on disk is untouched")
            .clicked()
        {
            cmds.push(AppCommand::RecorderDiscard);
        }
        let has_prev = !status.recording
            && std::path::Path::new(&format!("{}.prev", state.recorder_file.trim())).exists();
        if ui
            .add_enabled(has_prev, egui::Button::new("Revert"))
            .on_hover_text("Swap the take with its previous version (.prev)")
            .clicked()
        {
            cmds.push(AppCommand::RecorderRevert { file: state.recorder_file.clone() });
        }
        let mut monitor = state.recorder_monitor;
        if ui
            .checkbox(&mut monitor, "Monitor")
            .on_hover_text("Stream the merged result (take + live punches) to the lighting output while recording")
            .changed()
        {
            state.recorder_monitor = monitor;
            cmds.push(AppCommand::RecorderSetMonitor(monitor));
        }
        ui.separator();
        if ui
            .add_enabled(
                !status.recording && !state.recorder_file.trim().is_empty(),
                egui::Button::new("▶ Preview"),
            )
            .on_hover_text("Play the take through the lighting output")
            .clicked()
        {
            cmds.push(AppCommand::RecorderPreview { file: state.recorder_file.clone() });
        }
        if ui.button("⏹").on_hover_text("Stop preview playback").clicked() {
            cmds.push(AppCommand::RecorderStopPreview);
        }
        if ui
            .add_enabled(
                !status.recording && !state.recorder_file.trim().is_empty(),
                egui::Button::new("✏ Edit…"),
            )
            .on_hover_text("Open the take in the curve editor")
            .clicked()
        {
            state.open_take_editor = Some(state.recorder_file.clone());
        }
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Live input:").on_hover_text(
            "OSC: /dmx/{universe}/{channel} 0.0–1.0 — active as a bridge even when not recording",
        );
        let mut midi = state.recorder_midi_enabled;
        if ui
            .checkbox(&mut midi, "MIDI CC > universe")
            .on_hover_text("Map MIDI CC# to DMX channel (1-based) on this universe")
            .changed()
        {
            state.recorder_midi_enabled = midi;
        }
        let mut uni = state.recorder_midi_universe;
        if ui
            .add_enabled(
                midi,
                egui::DragValue::new(&mut uni).speed(1).range(1..=63999),
            )
            .changed()
        {
            state.recorder_midi_universe = uni;
        }
        if ui
            .button("Clear")
            .on_hover_text("Release every channel the live bridge holds")
            .clicked()
        {
            cmds.push(AppCommand::RecorderClearLive);
        }
    });

    ui.add_space(4.0);
    if status.recording {
        ui.label(format!(
            "🔴 {:>6.1}s   {} event(s)   {} channel(s) punched   {} packet(s)",
            status.elapsed_s, status.event_count, status.punched_count, status.rx_packets
        ));
    } else {
        ui.label(
            egui::RichText::new(
                "Listens for sACN (:5568) and Art-Net (:6454), unicast or broadcast. \
                 Recordings play back as DMX Show cues.",
            )
            .small()
            .weak(),
        );
    }
    if let Some(err) = &status.error {
        ui.colored_label(egui::Color32::LIGHT_RED, err);
    }

    for cmd in cmds {
        state.command_queue.push(cmd);
    }
}
