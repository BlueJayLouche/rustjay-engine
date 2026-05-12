use crate::control_gui::ControlGui;
use rustjay_core::HsbParams;

impl ControlGui {
    /// Build the Color tab
    pub(crate) fn build_color_tab(&mut self, ui: &imgui::Ui) {
        // Read base values from audio_routing (not modulated hsb_params)
        let (mut enabled, hue, sat, bright) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.color_enabled,
                state.audio_routing.base_hue,
                state.audio_routing.base_saturation,
                state.audio_routing.base_brightness,
            )
        };
        // Create HsbParams for convenience
        let mut hsb = HsbParams {
            hue_shift: hue,
            saturation: sat,
            brightness: bright,
        };

        ui.text("HSB Color Adjustment");
        ui.separator();

        // Enable/disable
        if ui.checkbox("Enable Color Adjustment", &mut enabled) {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.color_enabled = enabled;
        }

        ui.spacing();

        if enabled {
            // Hue shift
            ui.text("Hue Shift");
            if ui.slider_config("Hue", -180.0, 180.0)
                .display_format("%.0f°")
                .build(&mut hsb.hue_shift)
            {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.hsb_params.hue_shift = hsb.hue_shift;
                // Update audio routing base values
                state.audio_routing.update_base_values(hsb.hue_shift, hsb.saturation, hsb.brightness);
            }

            // Saturation
            ui.text("Saturation");
            if ui.slider_config("Saturation", 0.0, 2.0)
                .display_format("%.2fx")
                .build(&mut hsb.saturation)
            {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.hsb_params.saturation = hsb.saturation;
                // Update audio routing base values
                state.audio_routing.update_base_values(hsb.hue_shift, hsb.saturation, hsb.brightness);
            }

            // Brightness
            ui.text("Brightness");
            if ui.slider_config("Brightness", 0.0, 2.0)
                .display_format("%.2fx")
                .build(&mut hsb.brightness)
            {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.hsb_params.brightness = hsb.brightness;
                // Update audio routing base values
                state.audio_routing.update_base_values(hsb.hue_shift, hsb.saturation, hsb.brightness);
            }

            ui.spacing();
            ui.separator();
            ui.spacing();

            // Reset button
            if ui.button("Reset to Default") {
                hsb.reset();
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.hsb_params = hsb;
                // Update audio routing base values
                state.audio_routing.update_base_values(hsb.hue_shift, hsb.saturation, hsb.brightness);
            }

            ui.spacing();
            ui.separator();
            ui.spacing();

            // LFO Controls
            ui.text("LFO Modulation");

            if ui.button("Open LFO Window") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.lfo.show_window = true;
            }

            // Display active LFO count
            let active_lfos = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.lfo.bank.lfos.iter().filter(|b| b.enabled).count()
            };

            if active_lfos > 0 {
                ui.same_line();
                ui.text_colored(
                    [0.2, 0.8, 0.2, 1.0],
                    &format!("({} active)", active_lfos)
                );
            }
        } else {
            ui.text_disabled("Color adjustment is disabled");
        }
    }
}
