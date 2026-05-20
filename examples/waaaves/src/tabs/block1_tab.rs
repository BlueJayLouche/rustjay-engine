//! Block 1 tab — CH1, CH2, FB1 controls.

use super::*;

pub struct Block1Tab;

impl AnyGuiTab for Block1Tab {
    fn name(&self) -> &str { "Block 1" }

    fn draw(&mut self, ui: &imgui::Ui, app_state: &mut dyn std::any::Any, engine: &mut EngineState) {
        let state = app_state
            .downcast_mut::<WaaavesState>()
            .expect("Block1Tab expects WaaavesState");

        apply_pending_pick(state, engine);

        // ── CH1 Geometry ────────────────────────────────────────────────────
        if ui.collapsing_header("CH1 Geometry", imgui::TreeNodeFlags::DEFAULT_OPEN) {
            co(ui, engine, "Input##ch1", &mut state.block1.ch1_input_select, INPUT1_SELECT_OPTS);
            geometry_section(
                ui, engine, "ch1",
                &mut state.block1.ch1_x_displace,
                &mut state.block1.ch1_y_displace,
                &mut state.block1.ch1_z_displace,
                &mut state.block1.ch1_rotate,
                &mut state.block1.ch1_kaleidoscope_amount,
                &mut state.block1.ch1_kaleidoscope_slice,
                &mut state.block1.ch1_h_mirror,
                &mut state.block1.ch1_v_mirror,
                &mut state.block1.ch1_h_flip,
                &mut state.block1.ch1_v_flip,
                &mut state.block1.ch1_geo_overflow,
            );
            cb(ui, engine, "HD Aspect##ch1", "ch1_hd_aspect_on", &mut state.block1.ch1_hd_aspect_on);
        }

        // ── CH1 Color & Filters ─────────────────────────────────────────────
        if ui.collapsing_header("CH1 Color & Filters", imgui::TreeNodeFlags::empty()) {
            color_section(
                ui, engine, "ch1",
                &mut state.block1.ch1_hsb_attenuate_h,
                &mut state.block1.ch1_hsb_attenuate_s,
                &mut state.block1.ch1_hsb_attenuate_b,
                &mut state.block1.ch1_hue_invert,
                &mut state.block1.ch1_saturation_invert,
                &mut state.block1.ch1_bright_invert,
                &mut state.block1.ch1_rgb_invert,
                &mut state.block1.ch1_solarize,
                &mut state.block1.ch1_posterize_switch,
            );
            filter_section(
                ui, engine, "ch1",
                &mut state.block1.ch1_blur_amount,
                &mut state.block1.ch1_sharpen_amount,
                &mut state.block1.ch1_filters_boost,
            );
        }

        // ── CH2 Mix & Key ───────────────────────────────────────────────────
        if ui.collapsing_header("CH2 Mix & Key", imgui::TreeNodeFlags::empty()) {
            mix_key_section(
                ui, &mut state.pick_state, engine, "ch2",
                &mut state.block1.ch2_mix_amount,
                &mut state.block1.ch2_mix_type,
                &mut state.block1.ch2_mix_overflow,
                &mut state.block1.ch2_key_order,
                &mut state.block1.ch2_key_mode,
                &mut state.block1.ch2_key_threshold,
                &mut state.block1.ch2_key_soft,
                &mut state.block1.ch2_key_value_r,
                &mut state.block1.ch2_key_value_g,
                &mut state.block1.ch2_key_value_b,
                KeyTarget::Ch2,
            );
        }

        // ── CH2 Geometry ────────────────────────────────────────────────────
        if ui.collapsing_header("CH2 Geometry", imgui::TreeNodeFlags::empty()) {
            co(ui, engine, "Input##ch2", &mut state.block1.ch2_input_select, INPUT1_SELECT_OPTS);
            geometry_section(
                ui, engine, "ch2",
                &mut state.block1.ch2_x_displace,
                &mut state.block1.ch2_y_displace,
                &mut state.block1.ch2_z_displace,
                &mut state.block1.ch2_rotate,
                &mut state.block1.ch2_kaleidoscope_amount,
                &mut state.block1.ch2_kaleidoscope_slice,
                &mut state.block1.ch2_h_mirror,
                &mut state.block1.ch2_v_mirror,
                &mut state.block1.ch2_h_flip,
                &mut state.block1.ch2_v_flip,
                &mut state.block1.ch2_geo_overflow,
            );
            cb(ui, engine, "HD Aspect##ch2", "ch2_hd_aspect_on", &mut state.block1.ch2_hd_aspect_on);
        }

        // ── CH2 Color & Filters ─────────────────────────────────────────────
        if ui.collapsing_header("CH2 Color & Filters", imgui::TreeNodeFlags::empty()) {
            color_section(
                ui, engine, "ch2",
                &mut state.block1.ch2_hsb_attenuate_h,
                &mut state.block1.ch2_hsb_attenuate_s,
                &mut state.block1.ch2_hsb_attenuate_b,
                &mut state.block1.ch2_hue_invert,
                &mut state.block1.ch2_saturation_invert,
                &mut state.block1.ch2_bright_invert,
                &mut state.block1.ch2_rgb_invert,
                &mut state.block1.ch2_solarize,
                &mut state.block1.ch2_posterize_switch,
            );
            filter_section(
                ui, engine, "ch2",
                &mut state.block1.ch2_blur_amount,
                &mut state.block1.ch2_sharpen_amount,
                &mut state.block1.ch2_filters_boost,
            );
        }

        // ── FB1 Mix & Key ───────────────────────────────────────────────────
        if ui.collapsing_header("FB1 Mix & Key", imgui::TreeNodeFlags::empty()) {
            mix_key_section(
                ui, &mut state.pick_state, engine, "fb1",
                &mut state.block1.fb1_mix_amount,
                &mut state.block1.fb1_mix_type,
                &mut state.block1.fb1_mix_overflow,
                &mut state.block1.fb1_key_order,
                &mut state.block1.fb1_key_mode,
                &mut state.block1.fb1_key_threshold,
                &mut state.block1.fb1_key_soft,
                &mut state.block1.fb1_key_value_r,
                &mut state.block1.fb1_key_value_g,
                &mut state.block1.fb1_key_value_b,
                KeyTarget::Fb1,
            );
        }

        // ── FB1 Geometry ────────────────────────────────────────────────────
        if ui.collapsing_header("FB1 Geometry", imgui::TreeNodeFlags::empty()) {
            geometry_section(
                ui, engine, "fb1",
                &mut state.block1.fb1_x_displace,
                &mut state.block1.fb1_y_displace,
                &mut state.block1.fb1_z_displace,
                &mut state.block1.fb1_rotate,
                &mut state.block1.fb1_kaleidoscope_amount,
                &mut state.block1.fb1_kaleidoscope_slice,
                &mut state.block1.fb1_h_mirror,
                &mut state.block1.fb1_v_mirror,
                &mut state.block1.fb1_h_flip,
                &mut state.block1.fb1_v_flip,
                &mut state.block1.fb1_geo_overflow,
            );
            co(ui, engine, "Rotate Mode##fb1", &mut state.block1.fb1_rotate_mode, ROTATE_MODE_OPTS);
            sf(ui, engine, "Shear XX##fb1", "fb1_shear_xx", &mut state.block1.fb1_shear_xx, -2.0, 2.0);
            sf(ui, engine, "Shear XY##fb1", "fb1_shear_xy", &mut state.block1.fb1_shear_xy, -2.0, 2.0);
            sf(ui, engine, "Shear YX##fb1", "fb1_shear_yx", &mut state.block1.fb1_shear_yx, -2.0, 2.0);
            sf(ui, engine, "Shear YY##fb1", "fb1_shear_yy", &mut state.block1.fb1_shear_yy, -2.0, 2.0);
        }

        // ── FB1 Color ───────────────────────────────────────────────────────
        if ui.collapsing_header("FB1 Color", imgui::TreeNodeFlags::empty()) {
            sf(ui, engine, "Hue Offset##fb1", "fb1_hsb_offset_h", &mut state.block1.fb1_hsb_offset_h, -1.0, 1.0);
            sf(ui, engine, "Sat Offset##fb1", "fb1_hsb_offset_s", &mut state.block1.fb1_hsb_offset_s, -1.0, 1.0);
            sf(ui, engine, "Bri Offset##fb1", "fb1_hsb_offset_b", &mut state.block1.fb1_hsb_offset_b, -1.0, 1.0);
            sf(
                ui,
                engine,
                "Hue Attenuate##fb1",
                "fb1_hsb_attenuate_h",
                &mut state.block1.fb1_hsb_attenuate_h,
                0.0,
                2.0,
            );
            sf(
                ui,
                engine,
                "Sat Attenuate##fb1",
                "fb1_hsb_attenuate_s",
                &mut state.block1.fb1_hsb_attenuate_s,
                0.0,
                2.0,
            );
            sf(
                ui,
                engine,
                "Bri Attenuate##fb1",
                "fb1_hsb_attenuate_b",
                &mut state.block1.fb1_hsb_attenuate_b,
                0.0,
                2.0,
            );
            sf(ui, engine, "Hue PowMap##fb1", "fb1_hsb_powmap_h", &mut state.block1.fb1_hsb_powmap_h, 0.0, 4.0);
            sf(ui, engine, "Sat PowMap##fb1", "fb1_hsb_powmap_s", &mut state.block1.fb1_hsb_powmap_s, 0.0, 4.0);
            sf(ui, engine, "Bri PowMap##fb1", "fb1_hsb_powmap_b", &mut state.block1.fb1_hsb_powmap_b, 0.0, 4.0);
            sf(ui, engine, "Hue Shaper##fb1", "fb1_hue_shaper", &mut state.block1.fb1_hue_shaper, 0.0, 2.0);
            cb(ui, engine, "Hue Invert##fb1", "fb1_hue_invert", &mut state.block1.fb1_hue_invert);
            cb(
                ui,
                engine,
                "Sat Invert##fb1",
                "fb1_saturation_invert",
                &mut state.block1.fb1_saturation_invert,
            );
            cb(
                ui,
                engine,
                "Bri Invert##fb1",
                "fb1_bright_invert",
                &mut state.block1.fb1_bright_invert,
            );
            cb(
                ui,
                engine,
                "Posterize On##fb1",
                "fb1_posterize_switch",
                &mut state.block1.fb1_posterize_switch,
            );
        }

        // ── FB1 Filters ─────────────────────────────────────────────────────
        if ui.collapsing_header("FB1 Filters", imgui::TreeNodeFlags::empty()) {
            filter_section(
                ui, engine, "fb1",
                &mut state.block1.fb1_blur_amount,
                &mut state.block1.fb1_sharpen_amount,
                &mut state.block1.fb1_filters_boost,
            );
            sf(
                ui,
                engine,
                "Temporal 1 Amount##fb1",
                "fb1_temporal1_amount",
                &mut state.block1.fb1_temporal1_amount,
                0.0,
                1.0,
            );
            sf(
                ui,
                engine,
                "Temporal 1 Res##fb1",
                "fb1_temporal1_resonance",
                &mut state.block1.fb1_temporal1_resonance,
                0.0,
                1.0,
            );
            sf(
                ui,
                engine,
                "Temporal 2 Amount##fb1",
                "fb1_temporal2_amount",
                &mut state.block1.fb1_temporal2_amount,
                0.0,
                1.0,
            );
            sf(
                ui,
                engine,
                "Temporal 2 Res##fb1",
                "fb1_temporal2_resonance",
                &mut state.block1.fb1_temporal2_resonance,
                0.0,
                1.0,
            );
        }

        // ── FB1 Delay ───────────────────────────────────────────────────────
        if ui.collapsing_header("FB1 Delay", imgui::TreeNodeFlags::empty()) {
            delay_control(
                ui,
                engine,
                "fb1",
                "fb1_delay_time",
                &mut state.block1.fb1_delay_time,
                &mut state.block1.fb1_delay_time_sync,
                &mut state.block1.fb1_delay_time_division,
                state.max_delay_frames,
            );
        }
    }
}
