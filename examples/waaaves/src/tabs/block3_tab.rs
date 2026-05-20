//! Block 3 tab — B1/B2 re-process, matrix mixer, final mix.

use super::*;

pub struct Block3Tab;

impl AnyGuiTab for Block3Tab {
    fn name(&self) -> &str { "Block 3" }

    fn draw(&mut self, ui: &imgui::Ui, app_state: &mut dyn std::any::Any, engine: &mut EngineState) {
        let state = app_state
            .downcast_mut::<WaaavesState>()
            .expect("Block3Tab expects WaaavesState");

        // ── Block 1 Output Geometry ─────────────────────────────────────────
        if ui.collapsing_header("Block 1 Output Geometry", imgui::TreeNodeFlags::DEFAULT_OPEN) {
            geometry_section(
                ui, engine, "block1",
                &mut state.block3.block1_x_displace,
                &mut state.block3.block1_y_displace,
                &mut state.block3.block1_z_displace,
                &mut state.block3.block1_rotate,
                &mut state.block3.block1_kaleidoscope_amount,
                &mut state.block3.block1_kaleidoscope_slice,
                &mut state.block3.block1_h_mirror,
                &mut state.block3.block1_v_mirror,
                &mut state.block3.block1_h_flip,
                &mut state.block3.block1_v_flip,
                &mut state.block3.block1_geo_overflow,
            );
            co(ui, engine, "Rotate Mode##b1", &mut state.block3.block1_rotate_mode, ROTATE_MODE_OPTS);
            sf(ui, engine, "Shear XX##b1", "block1_shear_xx", &mut state.block3.block1_shear_xx, -2.0, 2.0);
            sf(ui, engine, "Shear XY##b1", "block1_shear_xy", &mut state.block3.block1_shear_xy, -2.0, 2.0);
            sf(ui, engine, "Shear YX##b1", "block1_shear_yx", &mut state.block3.block1_shear_yx, -2.0, 2.0);
            sf(ui, engine, "Shear YY##b1", "block1_shear_yy", &mut state.block3.block1_shear_yy, -2.0, 2.0);
        }

        // ── Block 1 Colorize ────────────────────────────────────────────────
        if ui.collapsing_header("Block 1 Colorize", imgui::TreeNodeFlags::empty()) {
            cb(ui, engine, "Colorize On##b1", "block1_colorize_switch", &mut state.block3.block1_colorize_switch);
            co(
                ui,
                engine,
                "Colorize Mode##b1",
                &mut state.block3.block1_colorize_hsb_rgb,
                COLORIZE_MODE_OPTS,
            );
            for i in 1..=5 {
                let (h, s, bri) = match i {
                    1 => (&mut state.block3.block1_colorize_band1_h, &mut state.block3.block1_colorize_band1_s, &mut state.block3.block1_colorize_band1_b),
                    2 => (&mut state.block3.block1_colorize_band2_h, &mut state.block3.block1_colorize_band2_s, &mut state.block3.block1_colorize_band2_b),
                    3 => (&mut state.block3.block1_colorize_band3_h, &mut state.block3.block1_colorize_band3_s, &mut state.block3.block1_colorize_band3_b),
                    4 => (&mut state.block3.block1_colorize_band4_h, &mut state.block3.block1_colorize_band4_s, &mut state.block3.block1_colorize_band4_b),
                    5 => (&mut state.block3.block1_colorize_band5_h, &mut state.block3.block1_colorize_band5_s, &mut state.block3.block1_colorize_band5_b),
                    _ => unreachable!(),
                };
                ui.text(format!("Band {i}"));
                sf(ui, engine, &format!("H##b1_band{i}"), &format!("block1_colorize_band{i}_h"), h, 0.0, 1.0);
                sf(ui, engine, &format!("S##b1_band{i}"), &format!("block1_colorize_band{i}_s"), s, 0.0, 1.0);
                sf(ui, engine, &format!("B##b1_band{i}"), &format!("block1_colorize_band{i}_b"), bri, 0.0, 1.0);
            }
        }

        // ── Block 1 Filters & Dither ────────────────────────────────────────
        if ui.collapsing_header("Block 1 Filters & Dither", imgui::TreeNodeFlags::empty()) {
            filter_section(
                ui, engine, "block1",
                &mut state.block3.block1_blur_amount,
                &mut state.block3.block1_sharpen_amount,
                &mut state.block3.block1_filters_boost,
            );
            cb(ui, engine, "Dither On##b1", "block1_dither_switch", &mut state.block3.block1_dither_switch);
            co(ui, engine, "Dither Type##b1", &mut state.block3.block1_dither_type, DITHER_TYPE_OPTS);
            sf(ui, engine, "Dither##b1", "block1_dither", &mut state.block3.block1_dither, 0.0, 32.0);
        }

        // ── Block 2 Output Geometry ─────────────────────────────────────────
        if ui.collapsing_header("Block 2 Output Geometry", imgui::TreeNodeFlags::empty()) {
            geometry_section(
                ui, engine, "block2",
                &mut state.block3.block2_x_displace,
                &mut state.block3.block2_y_displace,
                &mut state.block3.block2_z_displace,
                &mut state.block3.block2_rotate,
                &mut state.block3.block2_kaleidoscope_amount,
                &mut state.block3.block2_kaleidoscope_slice,
                &mut state.block3.block2_h_mirror,
                &mut state.block3.block2_v_mirror,
                &mut state.block3.block2_h_flip,
                &mut state.block3.block2_v_flip,
                &mut state.block3.block2_geo_overflow,
            );
            co(ui, engine, "Rotate Mode##b2", &mut state.block3.block2_rotate_mode, ROTATE_MODE_OPTS);
            sf(ui, engine, "Shear XX##b2", "block2_shear_xx", &mut state.block3.block2_shear_xx, -2.0, 2.0);
            sf(ui, engine, "Shear XY##b2", "block2_shear_xy", &mut state.block3.block2_shear_xy, -2.0, 2.0);
            sf(ui, engine, "Shear YX##b2", "block2_shear_yx", &mut state.block3.block2_shear_yx, -2.0, 2.0);
            sf(ui, engine, "Shear YY##b2", "block2_shear_yy", &mut state.block3.block2_shear_yy, -2.0, 2.0);
        }

        // ── Block 2 Colorize ────────────────────────────────────────────────
        if ui.collapsing_header("Block 2 Colorize", imgui::TreeNodeFlags::empty()) {
            cb(ui, engine, "Colorize On##b2", "block2_colorize_switch", &mut state.block3.block2_colorize_switch);
            co(
                ui,
                engine,
                "Colorize Mode##b2",
                &mut state.block3.block2_colorize_hsb_rgb,
                COLORIZE_MODE_OPTS,
            );
            for i in 1..=5 {
                let (h, s, bri) = match i {
                    1 => (&mut state.block3.block2_colorize_band1_h, &mut state.block3.block2_colorize_band1_s, &mut state.block3.block2_colorize_band1_b),
                    2 => (&mut state.block3.block2_colorize_band2_h, &mut state.block3.block2_colorize_band2_s, &mut state.block3.block2_colorize_band2_b),
                    3 => (&mut state.block3.block2_colorize_band3_h, &mut state.block3.block2_colorize_band3_s, &mut state.block3.block2_colorize_band3_b),
                    4 => (&mut state.block3.block2_colorize_band4_h, &mut state.block3.block2_colorize_band4_s, &mut state.block3.block2_colorize_band4_b),
                    5 => (&mut state.block3.block2_colorize_band5_h, &mut state.block3.block2_colorize_band5_s, &mut state.block3.block2_colorize_band5_b),
                    _ => unreachable!(),
                };
                ui.text(format!("Band {i}"));
                sf(ui, engine, &format!("H##b2_band{i}"), &format!("block2_colorize_band{i}_h"), h, 0.0, 1.0);
                sf(ui, engine, &format!("S##b2_band{i}"), &format!("block2_colorize_band{i}_s"), s, 0.0, 1.0);
                sf(ui, engine, &format!("B##b2_band{i}"), &format!("block2_colorize_band{i}_b"), bri, 0.0, 1.0);
            }
        }

        // ── Block 2 Filters & Dither ────────────────────────────────────────
        if ui.collapsing_header("Block 2 Filters & Dither", imgui::TreeNodeFlags::empty()) {
            filter_section(
                ui, engine, "block2",
                &mut state.block3.block2_blur_amount,
                &mut state.block3.block2_sharpen_amount,
                &mut state.block3.block2_filters_boost,
            );
            cb(ui, engine, "Dither On##b2", "block2_dither_switch", &mut state.block3.block2_dither_switch);
            co(ui, engine, "Dither Type##b2", &mut state.block3.block2_dither_type, DITHER_TYPE_OPTS);
            sf(ui, engine, "Dither##b2", "block2_dither", &mut state.block3.block2_dither, 0.0, 32.0);
        }

        // ── Matrix Mixer ────────────────────────────────────────────────────
        if ui.collapsing_header("Matrix Mixer", imgui::TreeNodeFlags::empty()) {
            co(ui, engine, "Mix Type##mat", &mut state.block3.matrix_mix_type, MIX_TYPE_OPTS);
            co(ui, engine, "Mix Overflow##mat", &mut state.block3.matrix_mix_overflow, MIX_OVERFLOW_OPTS);
            ui.text("R →");
            sf(ui, engine, "R→R", "matrix_mix_r_to_r", &mut state.block3.matrix_mix_r_to_r, -2.0, 2.0);
            sf(ui, engine, "R→G", "matrix_mix_r_to_g", &mut state.block3.matrix_mix_r_to_g, -2.0, 2.0);
            sf(ui, engine, "R→B", "matrix_mix_r_to_b", &mut state.block3.matrix_mix_r_to_b, -2.0, 2.0);
            ui.text("G →");
            sf(ui, engine, "G→R", "matrix_mix_g_to_r", &mut state.block3.matrix_mix_g_to_r, -2.0, 2.0);
            sf(ui, engine, "G→G", "matrix_mix_g_to_g", &mut state.block3.matrix_mix_g_to_g, -2.0, 2.0);
            sf(ui, engine, "G→B", "matrix_mix_g_to_b", &mut state.block3.matrix_mix_g_to_b, -2.0, 2.0);
            ui.text("B →");
            sf(ui, engine, "B→R", "matrix_mix_b_to_r", &mut state.block3.matrix_mix_b_to_r, -2.0, 2.0);
            sf(ui, engine, "B→G", "matrix_mix_b_to_g", &mut state.block3.matrix_mix_b_to_g, -2.0, 2.0);
            sf(ui, engine, "B→B", "matrix_mix_b_to_b", &mut state.block3.matrix_mix_b_to_b, -2.0, 2.0);
        }

        // ── Final Mix & Key ─────────────────────────────────────────────────
        if ui.collapsing_header("Final Mix & Key", imgui::TreeNodeFlags::empty()) {
            sf(ui, engine, "Final Mix Amount", "final_mix_amount", &mut state.block3.final_mix_amount, 0.0, 1.0);
            co(ui, engine, "Mix Type##final", &mut state.block3.final_mix_type, MIX_TYPE_OPTS);
            co(ui, engine, "Mix Overflow##final", &mut state.block3.final_mix_overflow, MIX_OVERFLOW_OPTS);
            co(ui, engine, "Key Order##final", &mut state.block3.final_key_order, KEY_ORDER_OPTS);
            sf(
                ui,
                engine,
                "Key Threshold##final",
                "final_key_threshold",
                &mut state.block3.final_key_threshold,
                0.0,
                1.0,
            );
            sf(
                ui,
                engine,
                "Key Soft##final",
                "final_key_soft",
                &mut state.block3.final_key_soft,
                0.0,
                1.0,
            );
            ui.text("Key Color");
            sf(ui, engine, "R##final_kr", "final_key_value_r", &mut state.block3.final_key_value_r, 0.0, 1.0);
            sf(ui, engine, "G##final_kg", "final_key_value_g", &mut state.block3.final_key_value_g, 0.0, 1.0);
            sf(ui, engine, "B##final_kb", "final_key_value_b", &mut state.block3.final_key_value_b, 0.0, 1.0);
        }

        // ── Final Dither ────────────────────────────────────────────────────
        if ui.collapsing_header("Final Dither", imgui::TreeNodeFlags::empty()) {
            cb(ui, engine, "Dither On##final", "final_dither_switch", &mut state.block3.final_dither_switch);
            co(ui, engine, "Dither Type##final", &mut state.block3.final_dither_type, DITHER_TYPE_OPTS);
            sf(ui, engine, "Dither##final", "final_dither", &mut state.block3.final_dither, 0.0, 32.0);
        }
    }
}
