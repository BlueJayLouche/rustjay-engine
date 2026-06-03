//! Transition tab — auto crossfade, beat-sync, and sequencer controls.

use rustjay_core::EngineState;
use rustjay_engine::prelude::AnyGuiTab;
use rustjay_mixer::{AutoCrossfade, BeatSyncCrossfade, Easing, TransitionStep};

pub struct TransitionTab {
    auto_target: f32,
    auto_duration: f32,
    auto_easing: i32,
    beat_target: f32,
    beat_beats: f32,
    seq_target: f32,
    seq_beats: f32,
    seq_looping: bool,
}

impl Default for TransitionTab {
    fn default() -> Self {
        Self {
            auto_target: 1.0,
            auto_duration: 2.0,
            auto_easing: 3, // EaseInOut
            beat_target: 1.0,
            beat_beats: 4.0,
            seq_target: 1.0,
            seq_beats: 2.0,
            seq_looping: false,
        }
    }
}

impl AnyGuiTab for TransitionTab {
    fn name(&self) -> &str {
        "Transitions"
    }

    fn draw(
        &mut self,
        ui: &imgui::Ui,
        app_state: &mut dyn std::any::Any,
        _engine: &mut EngineState,
    ) {
        let state = app_state.downcast_mut::<crate::MixerAppState>();
        let mixer = state.map(|s| s.mixer.clone());

        // ── Auto Crossfade ────────────────────────────────────────────────
        if ui.collapsing_header("Auto Crossfade", imgui::TreeNodeFlags::DEFAULT_OPEN) {
            ui.indent_by(8.0);

            ui.slider_config("Target", 0.0f32, 1.0f32).build(&mut self.auto_target);
            ui.slider_config("Duration (s)", 0.1f32, 10.0f32).build(&mut self.auto_duration);

            let easing_names = ["Linear", "EaseIn", "EaseOut", "EaseInOut"];
            let mut easing_idx = self.auto_easing as usize;
            ui.combo_simple_string("Easing", &mut easing_idx, &easing_names);
            self.auto_easing = easing_idx as i32;

            if ui.button("Start Auto Crossfade") {
                if let Some(ref m) = mixer {
                    let mut mixer = m.lock().unwrap();
                    let easing = match self.auto_easing {
                        1 => Easing::EaseIn,
                        2 => Easing::EaseOut,
                        3 => Easing::EaseInOut,
                        _ => Easing::Linear,
                    };
                    let current = mixer.crossfader;
                    mixer.auto = Some(AutoCrossfade::new(
                        current,
                        self.auto_target,
                        self.auto_duration,
                        easing,
                    ));
                }
            }
            ui.same_line();
            if ui.button("Stop") {
                if let Some(ref m) = mixer {
                    m.lock().unwrap().auto = None;
                }
            }

            ui.unindent_by(8.0);
        }

        ui.separator();

        // ── Beat-Sync Crossfade ───────────────────────────────────────────
        if ui.collapsing_header("Beat Sync", imgui::TreeNodeFlags::DEFAULT_OPEN) {
            ui.indent_by(8.0);

            ui.slider_config("Target", 0.0f32, 1.0f32).build(&mut self.beat_target);
            ui.slider_config("Beats", 0.5f32, 16.0f32).build(&mut self.beat_beats);

            if ui.button("Start Beat Sync") {
                if let Some(ref m) = mixer {
                    let mut mixer = m.lock().unwrap();
                    mixer.beat_sync = Some(BeatSyncCrossfade::new(self.beat_target, self.beat_beats));
                }
            }
            ui.same_line();
            if ui.button("Stop##beat") {
                if let Some(ref m) = mixer {
                    m.lock().unwrap().beat_sync = None;
                }
            }

            ui.unindent_by(8.0);
        }

        ui.separator();

        // ── Sequencer ─────────────────────────────────────────────────────
        if ui.collapsing_header("Sequencer", imgui::TreeNodeFlags::DEFAULT_OPEN) {
            ui.indent_by(8.0);

            ui.slider_config("Step Target", 0.0f32, 1.0f32).build(&mut self.seq_target);
            ui.slider_config("Step Beats", 0.5f32, 8.0f32).build(&mut self.seq_beats);

            if ui.button("Add Crossfade Step") {
                if let Some(ref m) = mixer {
                    let mut mixer = m.lock().unwrap();
                    mixer.sequencer.steps.push(TransitionStep::crossfade(self.seq_target, self.seq_beats));
                }
            }
            if ui.button("Add Hold Step") {
                if let Some(ref m) = mixer {
                    let mut mixer = m.lock().unwrap();
                    mixer.sequencer.steps.push(TransitionStep::hold(self.seq_beats));
                }
            }

            ui.checkbox("Loop", &mut self.seq_looping);

            if let Some(ref m) = mixer {
                let mixer = m.lock().unwrap();
                ui.text(format!("Steps: {}", mixer.sequencer.steps.len()));
                ui.text(format!("Playing: {}", mixer.sequencer.playing));
                ui.text(format!("Index: {}", mixer.sequencer.index));
            }

            if ui.button("Play") {
                if let Some(ref m) = mixer {
                    m.lock().unwrap().sequencer.play();
                }
            }
            ui.same_line();
            if ui.button("Stop##seq") {
                if let Some(ref m) = mixer {
                    m.lock().unwrap().sequencer.stop();
                }
            }
            ui.same_line();
            if ui.button("Clear Steps") {
                if let Some(ref m) = mixer {
                    m.lock().unwrap().sequencer.steps.clear();
                }
            }

            ui.unindent_by(8.0);
        }
    }
}
