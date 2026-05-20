//! Block 2 tab — Block 2 Input, FB2 controls.

use super::*;

pub struct Block2Tab;

impl AnyGuiTab for Block2Tab {
    fn name(&self) -> &str { "Block 2" }

    fn draw(&mut self, ui: &imgui::Ui, app_state: &mut dyn std::any::Any, engine: &mut EngineState) {
        let state = app_state
            .downcast_mut::<WaaavesState>()
            .expect("Block2Tab expects WaaavesState");

        // ── Block 2 Input ───────────────────────────────────────────────────
        if ui.collapsing_header("Block 2 Input", imgui::TreeNodeFlags::DEFAULT_OPEN) {
            co(
                ui,
                engine,
                "Input Select##b2",
                &mut state.block2.block2_input_select,
                BLOCK2_INPUT_SELECT_OPTS,
            );
            geometry_section(
                ui, engine, "block2_input",
                &mut state.block2.block2_input_x_displace,
                &mut state.block2.block2_input_y_displace,
                &mut state.block2.block2_input_z_displace,
                &mut state.block2.block2_input_rotate,
                &mut state.block2.block2_input_kaleidoscope_amount,
                &mut state.block2.block2_input_kaleidoscope_slice,
                &mut state.block2.block2_input_h_mirror,
                &mut state.block2.block2_input_v_mirror,
                &mut state.block2.block2_input_h_flip,
                &mut state.block2.block2_input_v_flip,
                &mut state.block2.block2_input_geo_overflow,
            );
            cb(
                ui,
                engine,
                "HD Aspect##b2_input",
                "block2_input_hd_aspect_on",
                &mut state.block2.block2_input_hd_aspect_on,
            );
        }

        // ── Block 2 Input Color ─────────────────────────────────────────────
        if ui.collapsing_header("Block 2 Input Color", imgui::TreeNodeFlags::empty()) {
            color_section(
                ui, engine, "block2_input",
                &mut state.block2.block2_input_hsb_attenuate_h,
                &mut state.block2.block2_input_hsb_attenuate_s,
                &mut state.block2.block2_input_hsb_attenuate_b,
                &mut state.block2.block2_input_hue_invert,
                &mut state.block2.block2_input_saturation_invert,
                &mut state.block2.block2_input_bright_invert,
                &mut state.block2.block2_input_rgb_invert,
                &mut state.block2.block2_input_solarize,
                &mut state.block2.block2_input_posterize_switch,
            );
            filter_section(
                ui, engine, "block2_input",
                &mut state.block2.block2_input_blur_amount,
                &mut state.block2.block2_input_sharpen_amount,
                &mut state.block2.block2_input_filters_boost,
            );
        }

        // ── FB2 Mix & Key ───────────────────────────────────────────────────
        if ui.collapsing_header("FB2 Mix & Key", imgui::TreeNodeFlags::empty()) {
            mix_key_section(
                ui, &mut state.pick_state, engine, "fb2",
                &mut state.block2.fb2_mix_amount,
                &mut state.block2.fb2_mix_type,
                &mut state.block2.fb2_mix_overflow,
                &mut state.block2.fb2_key_order,
                &mut state.block2.fb2_key_mode,
                &mut state.block2.fb2_key_threshold,
                &mut state.block2.fb2_key_soft,
                &mut state.block2.fb2_key_value_r,
                &mut state.block2.fb2_key_value_g,
                &mut state.block2.fb2_key_value_b,
                KeyTarget::Fb2,
            );
        }

        // ── FB2 Geometry ────────────────────────────────────────────────────
        if ui.collapsing_header("FB2 Geometry", imgui::TreeNodeFlags::empty()) {
            geometry_section(
                ui, engine, "fb2",
                &mut state.block2.fb2_x_displace,
                &mut state.block2.fb2_y_displace,
                &mut state.block2.fb2_z_displace,
                &mut state.block2.fb2_rotate,
                &mut state.block2.fb2_kaleidoscope_amount,
                &mut state.block2.fb2_kaleidoscope_slice,
                &mut state.block2.fb2_h_mirror,
                &mut state.block2.fb2_v_mirror,
                &mut state.block2.fb2_h_flip,
                &mut state.block2.fb2_v_flip,
                &mut state.block2.fb2_geo_overflow,
            );
            co(ui, engine, "Rotate Mode##fb2", &mut state.block2.fb2_rotate_mode, ROTATE_MODE_OPTS);
            sf(ui, engine, "Shear XX##fb2", "fb2_shear_xx", &mut state.block2.fb2_shear_xx, -2.0, 2.0);
            sf(ui, engine, "Shear XY##fb2", "fb2_shear_xy", &mut state.block2.fb2_shear_xy, -2.0, 2.0);
            sf(ui, engine, "Shear YX##fb2", "fb2_shear_yx", &mut state.block2.fb2_shear_yx, -2.0, 2.0);
            sf(ui, engine, "Shear YY##fb2", "fb2_shear_yy", &mut state.block2.fb2_shear_yy, -2.0, 2.0);
        }

        // ── FB2 Color ───────────────────────────────────────────────────────
        if ui.collapsing_header("FB2 Color", imgui::TreeNodeFlags::empty()) {
            sf(ui, engine, "Hue Offset##fb2", "fb2_hsb_offset_h", &mut state.block2.fb2_hsb_offset_h, -1.0, 1.0);
            sf(ui, engine, "Sat Offset##fb2", "fb2_hsb_offset_s", &mut state.block2.fb2_hsb_offset_s, -1.0, 1.0);
            sf(ui, engine, "Bri Offset##fb2", "fb2_hsb_offset_b", &mut state.block2.fb2_hsb_offset_b, -1.0, 1.0);
            sf(
                ui,
                engine,
                "Hue Attenuate##fb2",
                "fb2_hsb_attenuate_h",
                &mut state.block2.fb2_hsb_attenuate_h,
                0.0,
                2.0,
            );
            sf(
                ui,
                engine,
                "Sat Attenuate##fb2",
                "fb2_hsb_attenuate_s",
                &mut state.block2.fb2_hsb_attenuate_s,
                0.0,
                2.0,
            );
            sf(
                ui,
                engine,
                "Bri Attenuate##fb2",
                "fb2_hsb_attenuate_b",
                &mut state.block2.fb2_hsb_attenuate_b,
                0.0,
                2.0,
            );
            sf(ui, engine, "Hue PowMap##fb2", "fb2_hsb_powmap_h", &mut state.block2.fb2_hsb_powmap_h, 0.0, 4.0);
            sf(ui, engine, "Sat PowMap##fb2", "fb2_hsb_powmap_s", &mut state.block2.fb2_hsb_powmap_s, 0.0, 4.0);
            sf(ui, engine, "Bri PowMap##fb2", "fb2_hsb_powmap_b", &mut state.block2.fb2_hsb_powmap_b, 0.0, 4.0);
            sf(ui, engine, "Hue Shaper##fb2", "fb2_hue_shaper", &mut state.block2.fb2_hue_shaper, 0.0, 2.0);
            cb(ui, engine, "Hue Invert##fb2", "fb2_hue_invert", &mut state.block2.fb2_hue_invert);
            cb(
                ui,
                engine,
                "Sat Invert##fb2",
                "fb2_saturation_invert",
                &mut state.block2.fb2_saturation_invert,
            );
            cb(
                ui,
                engine,
                "Bri Invert##fb2",
                "fb2_bright_invert",
                &mut state.block2.fb2_bright_invert,
            );
            cb(
                ui,
                engine,
                "Posterize On##fb2",
                "fb2_posterize_switch",
                &mut state.block2.fb2_posterize_switch,
            );
        }

        // ── FB2 Filters ─────────────────────────────────────────────────────
        if ui.collapsing_header("FB2 Filters", imgui::TreeNodeFlags::empty()) {
            filter_section(
                ui, engine, "fb2",
                &mut state.block2.fb2_blur_amount,
                &mut state.block2.fb2_sharpen_amount,
                &mut state.block2.fb2_filters_boost,
            );
            sf(
                ui,
                engine,
                "Temporal 1 Amount##fb2",
                "fb2_temporal1_amount",
                &mut state.block2.fb2_temporal1_amount,
                0.0,
                1.0,
            );
            sf(
                ui,
                engine,
                "Temporal 1 Res##fb2",
                "fb2_temporal1_resonance",
                &mut state.block2.fb2_temporal1_resonance,
                0.0,
                1.0,
            );
            sf(
                ui,
                engine,
                "Temporal 2 Amount##fb2",
                "fb2_temporal2_amount",
                &mut state.block2.fb2_temporal2_amount,
                0.0,
                1.0,
            );
            sf(
                ui,
                engine,
                "Temporal 2 Res##fb2",
                "fb2_temporal2_resonance",
                &mut state.block2.fb2_temporal2_resonance,
                0.0,
                1.0,
            );
        }

        // ── FB2 Delay ───────────────────────────────────────────────────────
        if ui.collapsing_header("FB2 Delay", imgui::TreeNodeFlags::empty()) {
            delay_control(
                ui,
                engine,
                "fb2",
                "fb2_delay_time",
                &mut state.block2.fb2_delay_time,
                &mut state.block2.fb2_delay_time_sync,
                &mut state.block2.fb2_delay_time_division,
                state.max_delay_frames,
            );
        }
    }
}
