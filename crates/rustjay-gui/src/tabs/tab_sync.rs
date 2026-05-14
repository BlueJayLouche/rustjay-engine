use crate::control_gui::ControlGui;
#[cfg(feature = "link")]
use rustjay_core::LinkCommand;
#[cfg(feature = "prodj")]
use rustjay_core::ProDjCommand;

impl ControlGui {
    /// Build the Sync tab (Ableton Link + ProDJ Link).
    #[allow(unused_variables, unused_mut, unused_imports)]
    pub(crate) fn build_sync_tab(&mut self, ui: &imgui::Ui) {
        let (
            sync_source,
            mut link_enabled,
            link_peers,
            link_bpm,
            link_phase,
            mut link_quantum,
            link_playing,
            mut prodj_enabled,
            prodj_devices,
            prodj_master_bpm,
            prodj_artist,
            prodj_title,
        ) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.effective_sync_source().to_string(),
                state.link.enabled,
                state.link.num_peers,
                state.link.bpm,
                state.link.beat_phase,
                state.link.quantum,
                state.link.is_playing,
                state.prodj.enabled,
                state.prodj.devices.clone(),
                state.prodj.master_bpm,
                state.prodj.current_track_artist.clone(),
                state.prodj.current_track_title.clone(),
            )
        };

        // Suppress unused warnings when features are disabled.
        #[cfg(not(feature = "prodj"))]
        let _ = (&prodj_master_bpm, &prodj_artist, &prodj_title);

        ui.text_colored([0.0, 1.0, 1.0, 1.0], "Tempo Sync");
        ui.text(format!("Current source: {}", sync_source));
        ui.separator();

        // ── Ableton Link ──
        #[cfg(feature = "link")]
        {
            ui.text_colored([0.0, 1.0, 1.0, 1.0], "Ableton Link");
            if ui.checkbox("Enable Link", &mut link_enabled) {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.link_command = if link_enabled {
                    LinkCommand::Enable
                } else {
                    LinkCommand::Disable
                };
            }

            ui.text(format!("Peers: {}", link_peers));
            ui.text(format!("BPM: {:.2}", link_bpm));
            ui.text(format!("Playing: {}", if link_playing { "Yes" } else { "No" }));

            // Beat phase progress bar
            ui.text("Beat phase");
            imgui::ProgressBar::new(link_phase)
                .overlay_text(format!("{:.0}%", link_phase * 100.0))
                .build(ui);

            let mut quantum_f32 = link_quantum as f32;
            if ui.slider("Quantum", 1.0_f32, 16.0_f32, &mut quantum_f32) {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.link_command = LinkCommand::SetQuantum(quantum_f32 as f64);
            }

            ui.separator();
        }

        #[cfg(not(feature = "link"))]
        {
            ui.text_disabled("Ableton Link support not compiled in.");
            ui.text_disabled("Enable the 'link' feature to use Link sync.");
            ui.separator();
        }

        // ── ProDJ Link ──
        #[cfg(feature = "prodj")]
        {
            ui.text_colored([0.0, 1.0, 1.0, 1.0], "ProDJ Link");
            if ui.checkbox("Enable ProDJ Link", &mut prodj_enabled) {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.prodj_command = if prodj_enabled {
                    ProDjCommand::Start
                } else {
                    ProDjCommand::Stop
                };
            }

            ui.text(format!("Master BPM: {:.2}", prodj_master_bpm));

            if !prodj_artist.is_empty() || !prodj_title.is_empty() {
                ui.text(format!("Track: {} - {}", prodj_artist, prodj_title));
            }

            // Device list
            if !prodj_devices.is_empty() {
                ui.text("Discovered decks");
                for device in &prodj_devices {
                    let master_tag = if device.is_master { " [MASTER]" } else { "" };
                    let playing_tag = if device.is_playing { "▶" } else { "⏸" };
                    ui.text(format!(
                        "{} Deck {}: {}{} | BPM: {:.2}",
                        playing_tag,
                        device.device_id,
                        device.name,
                        master_tag,
                        device.bpm.unwrap_or(0.0)
                    ));
                }
            } else if prodj_enabled {
                ui.text_disabled("No devices discovered yet...");
            }

            ui.separator();
        }

        #[cfg(not(feature = "prodj"))]
        {
            ui.text_disabled("ProDJ Link support not compiled in.");
            ui.text_disabled("Enable the 'prodj' feature to use ProDJ sync.");
            ui.separator();
        }
    }
}
