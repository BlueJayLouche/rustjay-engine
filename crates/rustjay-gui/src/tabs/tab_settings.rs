use crate::control_gui::ControlGui;
use rustjay_core::OutputCommand;

impl ControlGui {
    /// Build the Settings tab
    pub(crate) fn build_settings_tab(&mut self, ui: &imgui::Ui) {
        ui.text("Application Settings");
        ui.separator();

        let mut ui_scale = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.ui_scale
        };

        ui.text("UI Scale:");
        if ui.slider("Scale", 0.5, 2.0, &mut ui_scale) {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.ui_scale = ui_scale;
        }

        ui.separator();
        ui.spacing();

        // Resolution settings with dropdown presets
        ui.text_colored([0.0, 1.0, 1.0, 1.0], "Resolution Settings");

        // Resolution preset dropdown
        let presets = [
            ("Custom", 0, 0),
            ("480p (640x480)", 640, 480),
            ("720p (1280x720)", 1280, 720),
            ("1080p (1920x1080)", 1920, 1080),
            ("1440p (2560x1440)", 2560, 1440),
            ("4K UHD (3840x2160)", 3840, 2160),
            ("Square 1:1 (1080x1080)", 1080, 1080),
            ("Vertical 9:16 (1080x1920)", 1080, 1920),
        ];

        let preset_names: Vec<&str> = presets.iter().map(|(name, _, _)| *name).collect();

        // Internal Resolution Section
        ui.text("Internal Resolution (Processing):");

        let (current_internal_w, current_internal_h) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.resolution.internal_width, state.resolution.internal_height)
        };

        // Find current preset index
        let mut internal_preset_idx = 0;
        for (i, (_, w, h)) in presets.iter().enumerate().skip(1) {
            if *w == current_internal_w && *h == current_internal_h {
                internal_preset_idx = i;
                break;
            }
        }

        let old_internal_preset = internal_preset_idx;
        if ui.combo_simple_string("Preset##internal", &mut internal_preset_idx, &preset_names)
            && internal_preset_idx != old_internal_preset && internal_preset_idx > 0 {
                let (_, w, h) = presets[internal_preset_idx];
                self.pending_internal_width = w;
                self.pending_internal_height = h;
            }

        // Manual input
        let mut w = self.pending_internal_width as i32;
        let mut h = self.pending_internal_height as i32;
        ui.text("Custom:");
        ui.input_int("Width##internal", &mut w).step(1).build();
        ui.input_int("Height##internal", &mut h).step(1).build();
        self.pending_internal_width = w.max(320) as u32;
        self.pending_internal_height = h.max(240) as u32;

        // Output Resolution Section
        ui.spacing();
        ui.text("Output Resolution (Display/NDI):");

        let (current_output_w, current_output_h) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.output_width, state.output_height)
        };

        // Find current preset index
        let mut output_preset_idx = 0;
        for (i, (_, w, h)) in presets.iter().enumerate().skip(1) {
            if *w == current_output_w && *h == current_output_h {
                output_preset_idx = i;
                break;
            }
        }

        let old_output_preset = output_preset_idx;
        if ui.combo_simple_string("Preset##output", &mut output_preset_idx, &preset_names)
            && output_preset_idx != old_output_preset && output_preset_idx > 0 {
                let (_, w, h) = presets[output_preset_idx];
                self.pending_output_width = w;
                self.pending_output_height = h;
            }

        // Manual input
        let mut ow = self.pending_output_width as i32;
        let mut oh = self.pending_output_height as i32;
        ui.text("Custom:");
        ui.input_int("Width##output", &mut ow).step(1).build();
        ui.input_int("Height##output", &mut oh).step(1).build();
        self.pending_output_width = ow.max(320) as u32;
        self.pending_output_height = oh.max(240) as u32;

        // Apply button
        ui.spacing();
        let _btn_color = ui.push_style_color(imgui::StyleColor::Button, [0.2, 0.7, 0.3, 1.0]);
        let _btn_hover = ui.push_style_color(imgui::StyleColor::ButtonHovered, [0.3, 0.8, 0.4, 1.0]);
        let _btn_active = ui.push_style_color(imgui::StyleColor::ButtonActive, [0.1, 0.6, 0.2, 1.0]);

        if ui.button_with_size("Apply Resolution Changes", [ui.content_region_avail()[0], 30.0]) {
            // Apply internal resolution
            {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.resolution.internal_width = self.pending_internal_width;
                state.resolution.internal_height = self.pending_internal_height;
                state.output_width = self.pending_output_width;
                state.output_height = self.pending_output_height;
            }
            // Signal resolution change command
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.output_command = OutputCommand::ResizeOutput;
            // Also signal to save settings
            state.save_settings_requested = true;
            log::info!("Resolution changed - Internal: {}x{}, Output: {}x{}",
                self.pending_internal_width, self.pending_internal_height,
                self.pending_output_width, self.pending_output_height);
        }

        ui.spacing();
        ui.separator();
        ui.spacing();

        ui.text("Keyboard Shortcuts:");
        ui.bullet_text("Shift+F - Toggle Fullscreen");
        ui.bullet_text("Shift+T - Tap Tempo");
        ui.bullet_text("Escape - Exit Application");

        ui.separator();

        // Performance section with FPS counter (from output window)
        ui.text_colored([0.0, 1.0, 1.0, 1.0], "Performance (Output Window)");

        // Get FPS from shared state (updated by WgpuEngine)
        let (fps, frame_time_ms) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.performance.fps, state.performance.frame_time_ms)
        };

        ui.text(format!("Output FPS: {:.1}", fps));
        ui.text(format!("Frame Time: {:.2} ms", frame_time_ms));

        ui.spacing();
        ui.text("Target FPS:");

        let fps_options = [24u32, 30, 48, 60, 90, 120];
        let fps_labels = ["24 fps", "30 fps", "48 fps", "60 fps (recommended)", "90 fps", "120 fps"];

        let target_fps_val = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.target_fps
        };

        let mut current_idx = fps_options.iter().position(|&f| f == target_fps_val).unwrap_or(3);
        if ui.combo_simple_string("##target_fps", &mut current_idx, &fps_labels) {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.target_fps = fps_options[current_idx];
            state.save_settings_requested = true;
        }

        ui.text_disabled("All textures use native BGRA format for optimal macOS performance.");

        ui.separator();

        // Save settings button
        ui.spacing();
        let _btn_color = ui.push_style_color(imgui::StyleColor::Button, [0.2, 0.5, 0.8, 1.0]);
        let _btn_hover = ui.push_style_color(imgui::StyleColor::ButtonHovered, [0.3, 0.6, 0.9, 1.0]);
        let _btn_active = ui.push_style_color(imgui::StyleColor::ButtonActive, [0.1, 0.4, 0.7, 1.0]);

        if ui.button_with_size("Save All Settings", [ui.content_region_avail()[0], 30.0]) {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.save_settings_requested = true;
            log::info!("Save settings requested from GUI");
        }

        ui.text_disabled("Settings are auto-saved on exit, or manually with this button.");
    }
}
