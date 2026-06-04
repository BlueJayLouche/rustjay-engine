use crate::control_gui::ControlGui;
use rustjay_core::HsbParams;

impl ControlGui {
    /// Build the Color tab
    #[allow(dead_code)] // legacy imgui tab; retained alongside the egui control GUI
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
            const HUE_COLOR: [f32; 4]   = [1.00, 0.55, 0.10, 1.0]; // warm orange — hue
            const SAT_COLOR: [f32; 4]   = [0.20, 0.85, 0.80, 1.0]; // cyan — saturation
            const BRITE_COLOR: [f32; 4] = [1.00, 0.95, 0.30, 1.0]; // yellow — brightness

            // Hue shift
            {
                let _grab  = ui.push_style_color(imgui::StyleColor::SliderGrab,       HUE_COLOR);
                let _grab_a = ui.push_style_color(imgui::StyleColor::SliderGrabActive,  HUE_COLOR);
                if ui.slider_config("Hue", -180.0, 180.0)
                    .display_format("%.0f°")
                    .build(&mut hsb.hue_shift)
                {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.hsb_params.hue_shift      = hsb.hue_shift;
                    state.hsb_param_bases.hue_shift = hsb.hue_shift;
                    state.audio_routing.update_base_values(hsb.hue_shift, hsb.saturation, hsb.brightness);
                }
            }
            ui.same_line();
            ui.text_colored(HUE_COLOR, format!("{:+.0}°", hsb.hue_shift));

            // Saturation
            {
                let _grab  = ui.push_style_color(imgui::StyleColor::SliderGrab,       SAT_COLOR);
                let _grab_a = ui.push_style_color(imgui::StyleColor::SliderGrabActive,  SAT_COLOR);
                if ui.slider_config("Saturation", 0.0, 2.0)
                    .display_format("%.2fx")
                    .build(&mut hsb.saturation)
                {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.hsb_params.saturation      = hsb.saturation;
                    state.hsb_param_bases.saturation = hsb.saturation;
                    state.audio_routing.update_base_values(hsb.hue_shift, hsb.saturation, hsb.brightness);
                }
            }
            ui.same_line();
            ui.text_colored(SAT_COLOR, format!("{:.2}x", hsb.saturation));

            // Brightness
            {
                let _grab  = ui.push_style_color(imgui::StyleColor::SliderGrab,       BRITE_COLOR);
                let _grab_a = ui.push_style_color(imgui::StyleColor::SliderGrabActive,  BRITE_COLOR);
                if ui.slider_config("Brightness", 0.0, 2.0)
                    .display_format("%.2fx")
                    .build(&mut hsb.brightness)
                {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.hsb_params.brightness      = hsb.brightness;
                    state.hsb_param_bases.brightness = hsb.brightness;
                    state.audio_routing.update_base_values(hsb.hue_shift, hsb.saturation, hsb.brightness);
                }
            }
            ui.same_line();
            ui.text_colored(BRITE_COLOR, format!("{:.2}x", hsb.brightness));

            ui.spacing();
            ui.separator();
            ui.spacing();

            // Reset button
            if ui.button("Reset to Default") {
                hsb.reset();
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.hsb_params       = hsb;
                state.hsb_param_bases  = hsb;
                state.audio_routing.update_base_values(hsb.hue_shift, hsb.saturation, hsb.brightness);
            }

            ui.spacing();
            ui.separator();
            ui.spacing();

            // LFO Controls
            ui.text("LFO Modulation");

            // Display active LFO count
            let active_lfos = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.lfo.bank.lfos.iter().filter(|b| b.enabled).count()
            };

            if active_lfos > 0 {
                ui.same_line();
                ui.text_colored(
                    [0.2, 0.8, 0.2, 1.0],
                    format!("({} active)", active_lfos)
                );
            }
        } else {
            ui.text_disabled("Color adjustment is disabled");
        }
    }
}
