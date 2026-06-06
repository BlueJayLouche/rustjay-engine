use crate::control_gui::ControlGui;
use rustjay_core::SyncSource;

impl ControlGui {
    /// Build the Sync tab — source selector + per-source details.
    #[allow(unused_variables, unused_mut, dead_code)] // legacy imgui tab; retained alongside egui GUI
    pub(crate) fn build_sync_tab(&mut self, ui: &imgui::Ui) {
        // ── Source selector ──────────────────────────────────────────────────
        let mut sync_source = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.sync_source
        };

        ui.text_colored([0.0, 1.0, 1.0, 1.0], "Sync Source");

        let sources: &[(&str, SyncSource)] = &[
            ("Audio / Tap Tempo", SyncSource::Audio),
            #[cfg(feature = "link")]
            ("Ableton Link", SyncSource::AbletonLink),
            #[cfg(feature = "prodj")]
            ("ProDJ Link", SyncSource::ProDj),
        ];

        for &(label, variant) in sources {
            let mut selected = sync_source == variant;
            if ui.radio_button(label, &mut selected, true) && selected {
                sync_source = variant;
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.sync_source = variant;
            }
        }

        ui.separator();

        // ── Per-source detail panels ─────────────────────────────────────────
        match sync_source {
            SyncSource::Audio => {
                let bpm = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio.bpm
                };
                ui.text_colored([0.0, 1.0, 1.0, 1.0], "Audio / Tap Tempo");
                ui.text(format!("BPM: {:.2}", bpm));
                ui.text_disabled("Use the Tap Tempo button or audio beat detection.");
            }

            #[cfg(feature = "link")]
            SyncSource::AbletonLink => {
                let (link_peers, link_bpm, link_phase, mut link_quantum, link_playing) = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        state.link.num_peers,
                        state.link.bpm,
                        state.link.beat_phase,
                        state.link.quantum,
                        state.link.is_playing,
                    )
                };

                ui.text_colored([0.0, 1.0, 1.0, 1.0], "Ableton Link");
                ui.text(format!("Peers: {}", link_peers));
                ui.text(format!("BPM: {:.2}", link_bpm));
                ui.text(format!(
                    "Playing: {}",
                    if link_playing { "Yes" } else { "No" }
                ));

                ui.text("Beat phase");
                imgui::ProgressBar::new(link_phase)
                    .overlay_text(format!("{:.0}%", link_phase * 100.0))
                    .build(ui);

                let mut quantum_f32 = link_quantum as f32;
                if ui.slider("Quantum", 1.0_f32, 16.0_f32, &mut quantum_f32) {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.link.quantum = quantum_f32 as f64;
                    #[cfg(feature = "link")]
                    {
                        state.link_command =
                            rustjay_core::LinkCommand::SetQuantum(quantum_f32 as f64);
                    }
                }
            }

            #[cfg(feature = "prodj")]
            SyncSource::ProDj => {
                let (prodj_devices, prodj_master_bpm, prodj_artist, prodj_title) = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        state.prodj.devices.clone(),
                        state.prodj.master_bpm,
                        state.prodj.current_track_artist.clone(),
                        state.prodj.current_track_title.clone(),
                    )
                };

                ui.text_colored([0.0, 1.0, 1.0, 1.0], "ProDJ Link");
                ui.text(format!("Master BPM: {:.2}", prodj_master_bpm));

                if !prodj_artist.is_empty() || !prodj_title.is_empty() {
                    ui.text(format!("Track: {} - {}", prodj_artist, prodj_title));
                }

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
                } else {
                    ui.text_disabled("No devices discovered yet...");
                }
            }

            // Fallback when a variant is selected but its feature is off (shouldn't
            // happen at runtime, but keeps the exhaustiveness check happy).
            #[allow(unreachable_patterns)]
            _ => {
                ui.text_disabled("Selected source is not compiled in.");
            }
        }

        // ── MIDI Timecode ────────────────────────────────────────────────────
        ui.separator();
        ui.text_colored([0.0, 1.0, 1.0, 1.0], "MIDI Timecode (MTC)");

        #[cfg(feature = "mtc")]
        {
            let (running, playing, position, source) = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                (
                    state.mtc.running,
                    state.mtc.playing,
                    state.mtc.position,
                    state.mtc.source_device.clone(),
                )
            };

            let (status_color, status_text) = if playing {
                ([0.0_f32, 1.0, 0.0, 1.0], "Playing")
            } else if running {
                ([1.0_f32, 0.8, 0.0, 1.0], "Stopped")
            } else {
                ([0.5_f32, 0.5, 0.5, 1.0], "No signal")
            };

            ui.text("Status:");
            ui.same_line();
            ui.text_colored(status_color, status_text);

            if !source.is_empty() {
                ui.text(format!("Source:    {}", source));
            }
            ui.text(format!(
                "Position:  {}  [{}]",
                position,
                position.frame_rate.name()
            ));
            ui.text(format!("Elapsed:   {:.3}s", position.as_seconds_f64()));
            ui.text_disabled("Listening on all MIDI ports automatically.");
        }

        #[cfg(not(feature = "mtc"))]
        {
            ui.text_disabled("MIDI Timecode support not compiled in.");
            ui.text_disabled("Enable the 'mtc' feature to receive MTC.");
        }
    }
}
