//! Waaaves GUI tabs — Block 1, Block 2, Block 3.

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
pub const DITHER_TYPE_OPTS: &[&str] = &["4×4", "8×8"];
pub const COLORIZE_MODE_OPTS: &[&str] = &["HSB", "RGB"];
pub const INPUT1_SELECT_OPTS: &[&str] = &["Input 1", "Input 2"];
pub const BLOCK2_INPUT_SELECT_OPTS: &[&str] = &["Block 1", "Input 1", "Input 2"];

/// Float slider that syncs the engine parameter base.
pub fn sf(ui: &imgui::Ui, engine: &mut EngineState, label: &str, id: &str, v: &mut f32, min: f32, max: f32) {
    if ui.slider_config(label, min, max).build(v) {
        engine.set_param_base(id, *v);
    }
}

/// Integer slider (state is i32, engine stores f32).
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

/// Key-color RGB sliders + Pick button.
pub fn key_color(
    ui: &imgui::Ui,
    pick_state: &mut PickState,
    engine: &mut EngineState,
    target: KeyTarget,
    prefix: &str,
    r: &mut f32,
    g: &mut f32,
    b: &mut f32,
) {
    // Apply pending pick result
    if let PickState::Pending { target: t } = *pick_state {
        if t == target {
            if let Some(rgb) = engine.picked_color.take() {
                *r = rgb[0];
                *g = rgb[1];
                *b = rgb[2];
                *pick_state = PickState::Idle;
            }
        }
    }

    sf(ui, engine, &format!("R##{prefix}_kr"), &format!("{prefix}_key_value_r"), r, 0.0, 1.0);
    sf(ui, engine, &format!("G##{prefix}_kg"), &format!("{prefix}_key_value_g"), g, 0.0, 1.0);
    sf(ui, engine, &format!("B##{prefix}_kb"), &format!("{prefix}_key_value_b"), b, 0.0, 1.0);

    let armed = matches!(*pick_state, PickState::Armed { target: t } if t == target);
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
    key_color(ui, pick_state, engine, target, prefix, key_r, key_g, key_b);
}
