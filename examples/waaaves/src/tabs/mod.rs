//! Waaaves GUI tabs — Block 1, Block 2, Block 3.

use crate::lfo_ui::{draw_lfo_dots, lfo_context_menu};
use crate::state::{KeyTarget, PickState, WaaavesState};
use rustjay_engine::prelude::*;

pub mod block1_tab;
pub mod block2_tab;
pub mod block3_tab;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

pub const GEO_OVERFLOW_OPTS: &[&str] = &["Clamp", "Toroid", "Mirror"];
pub const MIX_TYPE_OPTS: &[&str] = &[
    "Replace",
    "Add",
    "Multiply",
    "Screen",
    "Difference",
    "Overlay",
    "Lighten",
    "Darken",
    "Chroma Dist",
];
pub const MIX_OVERFLOW_OPTS: &[&str] = &["Clamp", "Toroid", "Mirror"];
pub const KEY_ORDER_OPTS: &[&str] = &["Key FG", "Key BG"];
pub const KEY_MODE_OPTS: &[&str] = &["Lumakey", "Chromakey"];
pub const ROTATE_MODE_OPTS: &[&str] = &["Normal", "Mode 1", "Mode 2"];
pub const DITHER_TYPE_OPTS: &[&str] = &[
    "Bayer 4×4", "Bayer 8×8", "Blue Noise", "White Noise",
    "IGN", "Scanlines", "Checkerboard", "Stripes",
    "Bit Crush", "1-Bit", "Pixel Sort", "Atkinson", "RGB Split",
];
pub const COLORIZE_MODE_OPTS: &[&str] = &["HSB", "RGB"];
pub const INPUT1_SELECT_OPTS: &[&str] = &["Input 1", "Input 2"];
pub const BLOCK2_INPUT_SELECT_OPTS: &[&str] = &["Block 1", "Input 1", "Input 2"];

/// Float slider that syncs the engine parameter base.
pub fn sf(ui: &imgui::Ui, engine: &mut EngineState, label: &str, id: &str, v: &mut f32, min: f32, max: f32) {
    if ui.slider_config(label, min, max).build(v) {
        engine.set_param_base(id, *v);
    }
    draw_lfo_dots(ui, id, &engine.lfo.bank);
    lfo_context_menu(ui, id, label, engine);
}

/// Integer slider (state is i32, engine stores f32).
#[allow(dead_code)] // helper kept for tabs that don't currently use an integer slider
pub fn si(ui: &imgui::Ui, engine: &mut EngineState, label: &str, id: &str, v: &mut i32, min: i32, max: i32) {
    if ui.slider_config(label, min, max).build(v) {
        engine.set_param_base(id, *v as f32);
    }
}

/// Bool checkbox.
pub fn cb(ui: &imgui::Ui, engine: &mut EngineState, label: &str, id: &str, v: &mut bool) {
    if ui.checkbox(label, v) {
        engine.set_param_base(id, if *v { 1.0 } else { 0.0 });
    }
}

/// Enum combo for i32 state fields.
pub fn co(ui: &imgui::Ui, _engine: &mut EngineState, label: &str, v: &mut i32, opts: &[&str]) {
    let mut idx = (*v as usize).min(opts.len().saturating_sub(1));
    if ui.combo_simple_string(label, &mut idx, opts) {
        *v = idx as i32;
    }
}

/// Delay-time control with optional beat-division sync.
pub fn delay_control(
    ui: &imgui::Ui,
    engine: &mut EngineState,
    label: &str,
    time_id: &str,
    time: &mut u32,
    sync: &mut bool,
    division: &mut i32,
    max_frames: u32,
) {
    cb(ui, engine, &format!("Tempo Sync##{label}"), &format!("{time_id}_sync"), sync);
    if *sync {
        let mut div = *division as usize;
        let div_names = ["1/32", "1/16", "1/8", "1/4", "1/2", "1", "2", "4"];
        if ui.combo_simple_string(&format!("Beat Division##{label}"), &mut div, &div_names) {
            *division = div as i32;
        }
    } else {
        let mut t = *time as i32;
        if ui.slider_config(&format!("Delay Frames##{label}"), 1, max_frames as i32).build(&mut t) {
            *time = t as u32;
            engine.set_param_base(time_id, *time as f32);
        }
    }
}

/// Consume a pending pixel-pick into the matching state field.
/// Must be called unconditionally at the top of each tab's draw() so that a
/// collapsed section never leaves pick_state stuck in Armed/Pending.
pub fn apply_pending_pick(state: &mut WaaavesState, engine: &mut EngineState) {
    // The renderer runs before the GUI each frame, so pick_request is already
    // consumed by the time the tab sees it — the Armed→Pending transition never
    // fires. Check Armed too so picked_color is not silently dropped.
    let target = match state.pick_state {
        PickState::Armed { target } | PickState::Pending { target } => target,
        PickState::Idle => return,
    };
    if let Some(rgb) = engine.picked_color.take() {
        let (r, g, b, prefix): (&mut f32, &mut f32, &mut f32, &str) = match target {
            KeyTarget::Ch2 => (
                &mut state.block1.ch2_key_value_r,
                &mut state.block1.ch2_key_value_g,
                &mut state.block1.ch2_key_value_b,
                "ch2",
            ),
            KeyTarget::Fb1 => (
                &mut state.block1.fb1_key_value_r,
                &mut state.block1.fb1_key_value_g,
                &mut state.block1.fb1_key_value_b,
                "fb1",
            ),
            KeyTarget::Fb2 => (
                &mut state.block2.fb2_key_value_r,
                &mut state.block2.fb2_key_value_g,
                &mut state.block2.fb2_key_value_b,
                "fb2",
            ),
            KeyTarget::Final => (
                &mut state.block3.final_key_value_r,
                &mut state.block3.final_key_value_g,
                &mut state.block3.final_key_value_b,
                "final",
            ),
        };
        *r = rgb[0];
        *g = rgb[1];
        *b = rgb[2];
        engine.set_param_base(&format!("{prefix}_key_value_r"), rgb[0]);
        engine.set_param_base(&format!("{prefix}_key_value_g"), rgb[1]);
        engine.set_param_base(&format!("{prefix}_key_value_b"), rgb[2]);
        state.pick_state = PickState::Idle;
    }
}

/// Key-color RGB sliders + Pick button.
/// When `key_mode == 0` (Lumakey), shows a single slider that drives all
/// channels together with a grey preview.
/// When `key_mode == 1` (Chromakey), shows a `color_edit3` picker plus the
/// individual R/G/B sliders so LFO modulation still works.
pub fn key_color(
    ui: &imgui::Ui,
    pick_state: &mut PickState,
    engine: &mut EngineState,
    target: KeyTarget,
    prefix: &str,
    key_mode: i32,
    r: &mut f32,
    g: &mut f32,
    b: &mut f32,
) {
    if key_mode == 0 {
        // Lumakey – single value driving all channels
        let mut val = *r;
        let label = &format!("Key Value##{prefix}_kv");
        let id = &format!("{prefix}_key_value_r");
        if ui.slider_config(label, 0.0, 1.0).build(&mut val) {
            *r = val;
            *g = val;
            *b = val;
            engine.set_param_base(&format!("{prefix}_key_value_r"), val);
            engine.set_param_base(&format!("{prefix}_key_value_g"), val);
            engine.set_param_base(&format!("{prefix}_key_value_b"), val);
        }
        draw_lfo_dots(ui, id, &engine.lfo.bank);
        lfo_context_menu(ui, id, label, engine);

        ui.same_line();
        ui.color_button(&format!("Preview##{prefix}_preview"), [val, val, val, 1.0]);
    } else {
        // Chromakey – full color picker + individual sliders for LFO
        let mut color = [*r, *g, *b];
        if ui.color_edit3(&format!("##{prefix}_ce"), &mut color) {
            *r = color[0];
            *g = color[1];
            *b = color[2];
            engine.set_param_base(&format!("{prefix}_key_value_r"), color[0]);
            engine.set_param_base(&format!("{prefix}_key_value_g"), color[1]);
            engine.set_param_base(&format!("{prefix}_key_value_b"), color[2]);
        }

        sf(ui, engine, &format!("R##{prefix}_kr"), &format!("{prefix}_key_value_r"), r, 0.0, 1.0);
        sf(ui, engine, &format!("G##{prefix}_kg"), &format!("{prefix}_key_value_g"), g, 0.0, 1.0);
        sf(ui, engine, &format!("B##{prefix}_kb"), &format!("{prefix}_key_value_b"), b, 0.0, 1.0);
    }

    let armed = matches!(*pick_state, PickState::Armed { target: t } if t == target);
    let pending = matches!(*pick_state, PickState::Pending { target: t } if t == target);
    let btn = if armed { "Pick ⊘ (armed)" } else { "Pick ⊕" };
    if ui.button(btn) {
        *pick_state = if armed {
            PickState::Idle
        } else {
            PickState::Armed { target }
        };
    }
    if armed {
        ui.same_line();
        ui.text_disabled("Click preview to sample");
    }
    if pending {
        ui.same_line();
        ui.text_colored([1.0, 0.8, 0.2, 1.0], "Capturing…");
    }
}

/// Standard geometry section (displace, rotate, kaleidoscope, overflow, mirrors, flips).
pub fn geometry_section(
    ui: &imgui::Ui,
    engine: &mut EngineState,
    prefix: &str,
    x: &mut f32,
    y: &mut f32,
    z: &mut f32,
    rot: &mut f32,
    kaleido_amt: &mut f32,
    kaleido_slice: &mut f32,
    h_mirror: &mut bool,
    v_mirror: &mut bool,
    h_flip: &mut bool,
    v_flip: &mut bool,
    geo_overflow: &mut i32,
) {
    sf(ui, engine, &format!("X Displace##{prefix}"), &format!("{prefix}_x_displace"), x, -2.0, 2.0);
    sf(ui, engine, &format!("Y Displace##{prefix}"), &format!("{prefix}_y_displace"), y, -2.0, 2.0);
    sf(ui, engine, &format!("Zoom##{prefix}"), &format!("{prefix}_z_displace"), z, 0.0, 4.0);
    sf(
        ui,
        engine,
        &format!("Rotate##{prefix}"),
        &format!("{prefix}_rotate"),
        rot,
        -std::f32::consts::TAU,
        std::f32::consts::TAU,
    );
    sf(
        ui,
        engine,
        &format!("Kaleidoscope##{prefix}"),
        &format!("{prefix}_kaleidoscope_amount"),
        kaleido_amt,
        0.0,
        1.0,
    );
    sf(
        ui,
        engine,
        &format!("Kaleido Slice##{prefix}"),
        &format!("{prefix}_kaleidoscope_slice"),
        kaleido_slice,
        -std::f32::consts::PI,
        std::f32::consts::PI,
    );
    co(ui, engine, &format!("Overflow##{prefix}"), geo_overflow, GEO_OVERFLOW_OPTS);
    cb(ui, engine, &format!("H Mirror##{prefix}"), &format!("{prefix}_h_mirror"), h_mirror);
    cb(ui, engine, &format!("V Mirror##{prefix}"), &format!("{prefix}_v_mirror"), v_mirror);
    cb(ui, engine, &format!("H Flip##{prefix}"), &format!("{prefix}_h_flip"), h_flip);
    cb(ui, engine, &format!("V Flip##{prefix}"), &format!("{prefix}_v_flip"), v_flip);
}

/// Standard color section (HSB attenuate, inverts, posterize).
pub fn color_section(
    ui: &imgui::Ui,
    engine: &mut EngineState,
    prefix: &str,
    hsb_h: &mut f32,
    hsb_s: &mut f32,
    hsb_b: &mut f32,
    hue_inv: &mut bool,
    sat_inv: &mut bool,
    bri_inv: &mut bool,
    rgb_inv: &mut bool,
    solarize: &mut bool,
    posterize_sw: &mut bool,
) {
    sf(
        ui,
        engine,
        &format!("Hue Attenuate##{prefix}"),
        &format!("{prefix}_hsb_attenuate_h"),
        hsb_h,
        0.0,
        2.0,
    );
    sf(
        ui,
        engine,
        &format!("Sat Attenuate##{prefix}"),
        &format!("{prefix}_hsb_attenuate_s"),
        hsb_s,
        0.0,
        2.0,
    );
    sf(
        ui,
        engine,
        &format!("Bri Attenuate##{prefix}"),
        &format!("{prefix}_hsb_attenuate_b"),
        hsb_b,
        0.0,
        2.0,
    );
    cb(ui, engine, &format!("Hue Invert##{prefix}"), &format!("{prefix}_hue_invert"), hue_inv);
    cb(
        ui,
        engine,
        &format!("Sat Invert##{prefix}"),
        &format!("{prefix}_saturation_invert"),
        sat_inv,
    );
    cb(
        ui,
        engine,
        &format!("Bri Invert##{prefix}"),
        &format!("{prefix}_bright_invert"),
        bri_inv,
    );
    cb(ui, engine, &format!("RGB Invert##{prefix}"), &format!("{prefix}_rgb_invert"), rgb_inv);
    cb(ui, engine, &format!("Solarize##{prefix}"), &format!("{prefix}_solarize"), solarize);
    cb(
        ui,
        engine,
        &format!("Posterize On##{prefix}"),
        &format!("{prefix}_posterize_switch"),
        posterize_sw,
    );
}

/// Standard filter section (blur, sharpen, boost).
pub fn filter_section(
    ui: &imgui::Ui,
    engine: &mut EngineState,
    prefix: &str,
    blur_amt: &mut f32,
    sharpen_amt: &mut f32,
    boost: &mut f32,
) {
    sf(
        ui,
        engine,
        &format!("Blur Amount##{prefix}"),
        &format!("{prefix}_blur_amount"),
        blur_amt,
        0.0,
        1.0,
    );
    sf(
        ui,
        engine,
        &format!("Sharpen Amount##{prefix}"),
        &format!("{prefix}_sharpen_amount"),
        sharpen_amt,
        0.0,
        1.0,
    );
    sf(
        ui,
        engine,
        &format!("Filters Boost##{prefix}"),
        &format!("{prefix}_filters_boost"),
        boost,
        0.0,
        1.0,
    );
}

/// Mix & Key section (amount, type, overflow, order, mode, threshold, soft, key color).
pub fn mix_key_section(
    ui: &imgui::Ui,
    pick_state: &mut PickState,
    engine: &mut EngineState,
    prefix: &str,
    mix_amt: &mut f32,
    mix_type: &mut i32,
    mix_overflow: &mut i32,
    key_order: &mut i32,
    key_mode: &mut i32,
    key_thr: &mut f32,
    key_soft: &mut f32,
    key_r: &mut f32,
    key_g: &mut f32,
    key_b: &mut f32,
    target: KeyTarget,
) {
    sf(
        ui,
        engine,
        &format!("Mix Amount##{prefix}"),
        &format!("{prefix}_mix_amount"),
        mix_amt,
        0.0,
        1.0,
    );
    co(ui, engine, &format!("Mix Type##{prefix}"), mix_type, MIX_TYPE_OPTS);
    co(ui, engine, &format!("Mix Overflow##{prefix}"), mix_overflow, MIX_OVERFLOW_OPTS);
    co(ui, engine, &format!("Key Order##{prefix}"), key_order, KEY_ORDER_OPTS);
    co(ui, engine, &format!("Key Mode##{prefix}"), key_mode, KEY_MODE_OPTS);
    sf(
        ui,
        engine,
        &format!("Key Threshold##{prefix}"),
        &format!("{prefix}_key_threshold"),
        key_thr,
        0.0,
        1.0,
    );
    sf(
        ui,
        engine,
        &format!("Key Soft##{prefix}"),
        &format!("{prefix}_key_soft"),
        key_soft,
        0.0,
        1.0,
    );
    ui.text("Key Color");
    key_color(ui, pick_state, engine, target, prefix, *key_mode, key_r, key_g, key_b);
}
/// Sync all waaaves parameter bases from state to engine.
pub fn sync_all_params(state: &WaaavesState, engine: &mut EngineState) {
    let ids: Vec<String> = engine.param_descriptors.iter().map(|d| d.id.clone()).collect();
    for id in ids {
        let value = match id.as_str() {
            "ch1_x_displace" => state.block1.ch1_x_displace,
            "ch1_y_displace" => state.block1.ch1_y_displace,
            "ch1_z_displace" => state.block1.ch1_z_displace,
            "ch1_rotate" => state.block1.ch1_rotate,
            "ch1_hsb_attenuate_h" => state.block1.ch1_hsb_attenuate_h,
            "ch1_hsb_attenuate_s" => state.block1.ch1_hsb_attenuate_s,
            "ch1_hsb_attenuate_b" => state.block1.ch1_hsb_attenuate_b,
            "ch1_kaleidoscope_amount" => state.block1.ch1_kaleidoscope_amount,
            "ch1_kaleidoscope_slice" => state.block1.ch1_kaleidoscope_slice,
            "ch1_blur_amount" => state.block1.ch1_blur_amount,
            "ch1_sharpen_amount" => state.block1.ch1_sharpen_amount,
            "ch1_filters_boost" => state.block1.ch1_filters_boost,
            "ch2_mix_amount" => state.block1.ch2_mix_amount,
            "ch2_key_threshold" => state.block1.ch2_key_threshold,
            "ch2_key_soft" => state.block1.ch2_key_soft,
            "ch2_x_displace" => state.block1.ch2_x_displace,
            "ch2_y_displace" => state.block1.ch2_y_displace,
            "ch2_z_displace" => state.block1.ch2_z_displace,
            "ch2_rotate" => state.block1.ch2_rotate,
            "ch2_hsb_attenuate_h" => state.block1.ch2_hsb_attenuate_h,
            "ch2_hsb_attenuate_s" => state.block1.ch2_hsb_attenuate_s,
            "ch2_hsb_attenuate_b" => state.block1.ch2_hsb_attenuate_b,
            "ch2_kaleidoscope_amount" => state.block1.ch2_kaleidoscope_amount,
            "ch2_kaleidoscope_slice" => state.block1.ch2_kaleidoscope_slice,
            "ch2_blur_amount" => state.block1.ch2_blur_amount,
            "ch2_sharpen_amount" => state.block1.ch2_sharpen_amount,
            "ch2_filters_boost" => state.block1.ch2_filters_boost,
            "fb1_mix_amount" => state.block1.fb1_mix_amount,
            "fb1_key_threshold" => state.block1.fb1_key_threshold,
            "fb1_key_soft" => state.block1.fb1_key_soft,
            "fb1_x_displace" => state.block1.fb1_x_displace,
            "fb1_y_displace" => state.block1.fb1_y_displace,
            "fb1_z_displace" => state.block1.fb1_z_displace,
            "fb1_rotate" => state.block1.fb1_rotate,
            "fb1_shear_xx" => state.block1.fb1_shear_xx,
            "fb1_shear_xy" => state.block1.fb1_shear_xy,
            "fb1_shear_yx" => state.block1.fb1_shear_yx,
            "fb1_shear_yy" => state.block1.fb1_shear_yy,
            "fb1_kaleidoscope_amount" => state.block1.fb1_kaleidoscope_amount,
            "fb1_kaleidoscope_slice" => state.block1.fb1_kaleidoscope_slice,
            "fb1_hsb_offset_h" => state.block1.fb1_hsb_offset_h,
            "fb1_hsb_offset_s" => state.block1.fb1_hsb_offset_s,
            "fb1_hsb_offset_b" => state.block1.fb1_hsb_offset_b,
            "fb1_hsb_attenuate_h" => state.block1.fb1_hsb_attenuate_h,
            "fb1_hsb_attenuate_s" => state.block1.fb1_hsb_attenuate_s,
            "fb1_hsb_attenuate_b" => state.block1.fb1_hsb_attenuate_b,
            "fb1_hsb_powmap_h" => state.block1.fb1_hsb_powmap_h,
            "fb1_hsb_powmap_s" => state.block1.fb1_hsb_powmap_s,
            "fb1_hsb_powmap_b" => state.block1.fb1_hsb_powmap_b,
            "fb1_hue_shaper" => state.block1.fb1_hue_shaper,
            "fb1_blur_amount" => state.block1.fb1_blur_amount,
            "fb1_sharpen_amount" => state.block1.fb1_sharpen_amount,
            "fb1_filters_boost" => state.block1.fb1_filters_boost,
            "block2_input_x_displace" => state.block2.block2_input_x_displace,
            "block2_input_y_displace" => state.block2.block2_input_y_displace,
            "block2_input_z_displace" => state.block2.block2_input_z_displace,
            "block2_input_rotate" => state.block2.block2_input_rotate,
            "block2_input_hsb_attenuate_h" => state.block2.block2_input_hsb_attenuate_h,
            "block2_input_hsb_attenuate_s" => state.block2.block2_input_hsb_attenuate_s,
            "block2_input_hsb_attenuate_b" => state.block2.block2_input_hsb_attenuate_b,
            "block2_input_kaleidoscope_amount" => state.block2.block2_input_kaleidoscope_amount,
            "block2_input_kaleidoscope_slice" => state.block2.block2_input_kaleidoscope_slice,
            "block2_input_blur_amount" => state.block2.block2_input_blur_amount,
            "block2_input_sharpen_amount" => state.block2.block2_input_sharpen_amount,
            "fb2_mix_amount" => state.block2.fb2_mix_amount,
            "fb2_key_threshold" => state.block2.fb2_key_threshold,
            "fb2_key_soft" => state.block2.fb2_key_soft,
            "fb2_x_displace" => state.block2.fb2_x_displace,
            "fb2_y_displace" => state.block2.fb2_y_displace,
            "fb2_z_displace" => state.block2.fb2_z_displace,
            "fb2_rotate" => state.block2.fb2_rotate,
            "fb2_shear_xx" => state.block2.fb2_shear_xx,
            "fb2_shear_xy" => state.block2.fb2_shear_xy,
            "fb2_shear_yx" => state.block2.fb2_shear_yx,
            "fb2_shear_yy" => state.block2.fb2_shear_yy,
            "fb2_kaleidoscope_amount" => state.block2.fb2_kaleidoscope_amount,
            "fb2_kaleidoscope_slice" => state.block2.fb2_kaleidoscope_slice,
            "fb2_hsb_offset_h" => state.block2.fb2_hsb_offset_h,
            "fb2_hsb_offset_s" => state.block2.fb2_hsb_offset_s,
            "fb2_hsb_offset_b" => state.block2.fb2_hsb_offset_b,
            "fb2_hsb_attenuate_h" => state.block2.fb2_hsb_attenuate_h,
            "fb2_hsb_attenuate_s" => state.block2.fb2_hsb_attenuate_s,
            "fb2_hsb_attenuate_b" => state.block2.fb2_hsb_attenuate_b,
            "fb2_hsb_powmap_h" => state.block2.fb2_hsb_powmap_h,
            "fb2_hsb_powmap_s" => state.block2.fb2_hsb_powmap_s,
            "fb2_hsb_powmap_b" => state.block2.fb2_hsb_powmap_b,
            "fb2_hue_shaper" => state.block2.fb2_hue_shaper,
            "fb2_blur_amount" => state.block2.fb2_blur_amount,
            "fb2_sharpen_amount" => state.block2.fb2_sharpen_amount,
            "fb2_filters_boost" => state.block2.fb2_filters_boost,
            "block1_x_displace" => state.block3.block1_x_displace,
            "block1_y_displace" => state.block3.block1_y_displace,
            "block1_z_displace" => state.block3.block1_z_displace,
            "block1_rotate" => state.block3.block1_rotate,
            "block1_shear_xx" => state.block3.block1_shear_xx,
            "block1_shear_xy" => state.block3.block1_shear_xy,
            "block1_shear_yx" => state.block3.block1_shear_yx,
            "block1_shear_yy" => state.block3.block1_shear_yy,
            "block1_kaleidoscope_amount" => state.block3.block1_kaleidoscope_amount,
            "block1_kaleidoscope_slice" => state.block3.block1_kaleidoscope_slice,
            "block1_colorize_band1_h" => state.block3.block1_colorize_band1_h,
            "block1_colorize_band1_s" => state.block3.block1_colorize_band1_s,
            "block1_colorize_band1_b" => state.block3.block1_colorize_band1_b,
            "block1_colorize_band2_h" => state.block3.block1_colorize_band2_h,
            "block1_colorize_band2_s" => state.block3.block1_colorize_band2_s,
            "block1_colorize_band2_b" => state.block3.block1_colorize_band2_b,
            "block1_colorize_band3_h" => state.block3.block1_colorize_band3_h,
            "block1_colorize_band3_s" => state.block3.block1_colorize_band3_s,
            "block1_colorize_band3_b" => state.block3.block1_colorize_band3_b,
            "block1_colorize_band4_h" => state.block3.block1_colorize_band4_h,
            "block1_colorize_band4_s" => state.block3.block1_colorize_band4_s,
            "block1_colorize_band4_b" => state.block3.block1_colorize_band4_b,
            "block1_colorize_band5_h" => state.block3.block1_colorize_band5_h,
            "block1_colorize_band5_s" => state.block3.block1_colorize_band5_s,
            "block1_colorize_band5_b" => state.block3.block1_colorize_band5_b,
            "block2_x_displace" => state.block3.block2_x_displace,
            "block2_y_displace" => state.block3.block2_y_displace,
            "block2_z_displace" => state.block3.block2_z_displace,
            "block2_rotate" => state.block3.block2_rotate,
            "block2_shear_xx" => state.block3.block2_shear_xx,
            "block2_shear_xy" => state.block3.block2_shear_xy,
            "block2_shear_yx" => state.block3.block2_shear_yx,
            "block2_shear_yy" => state.block3.block2_shear_yy,
            "block2_kaleidoscope_amount" => state.block3.block2_kaleidoscope_amount,
            "block2_kaleidoscope_slice" => state.block3.block2_kaleidoscope_slice,
            "block2_colorize_band1_h" => state.block3.block2_colorize_band1_h,
            "block2_colorize_band1_s" => state.block3.block2_colorize_band1_s,
            "block2_colorize_band1_b" => state.block3.block2_colorize_band1_b,
            "block2_colorize_band2_h" => state.block3.block2_colorize_band2_h,
            "block2_colorize_band2_s" => state.block3.block2_colorize_band2_s,
            "block2_colorize_band2_b" => state.block3.block2_colorize_band2_b,
            "block2_colorize_band3_h" => state.block3.block2_colorize_band3_h,
            "block2_colorize_band3_s" => state.block3.block2_colorize_band3_s,
            "block2_colorize_band3_b" => state.block3.block2_colorize_band3_b,
            "block2_colorize_band4_h" => state.block3.block2_colorize_band4_h,
            "block2_colorize_band4_s" => state.block3.block2_colorize_band4_s,
            "block2_colorize_band4_b" => state.block3.block2_colorize_band4_b,
            "block2_colorize_band5_h" => state.block3.block2_colorize_band5_h,
            "block2_colorize_band5_s" => state.block3.block2_colorize_band5_s,
            "block2_colorize_band5_b" => state.block3.block2_colorize_band5_b,
            "matrix_mix_r_to_r" => state.block3.matrix_mix_r_to_r,
            "matrix_mix_r_to_g" => state.block3.matrix_mix_r_to_g,
            "matrix_mix_r_to_b" => state.block3.matrix_mix_r_to_b,
            "matrix_mix_g_to_r" => state.block3.matrix_mix_g_to_r,
            "matrix_mix_g_to_g" => state.block3.matrix_mix_g_to_g,
            "matrix_mix_g_to_b" => state.block3.matrix_mix_g_to_b,
            "matrix_mix_b_to_r" => state.block3.matrix_mix_b_to_r,
            "matrix_mix_b_to_g" => state.block3.matrix_mix_b_to_g,
            "matrix_mix_b_to_b" => state.block3.matrix_mix_b_to_b,
            "final_mix_amount" => state.block3.final_mix_amount,
            _ => continue,
        };
        engine.set_param_base(&id, value);
    }
}
