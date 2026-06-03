use crate::state::WaaavesState;
use rustjay_engine::EngineState;

#[repr(C, align(16))]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Align16 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    _pad: f32,
}

impl Align16 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z, _pad: 0.0 }
    }
}

// ---------------------------------------------------------------------------
// Block A – matches original Block1Uniforms layout (544 bytes)
// ---------------------------------------------------------------------------
#[repr(C, align(16))]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BlockAUniforms {
    // Resolution
    pub width: f32,
    pub height: f32,
    pub inv_width: f32,
    pub inv_height: f32,

    // Input texture dimensions
    pub ch1_input_width: f32,
    pub ch1_input_height: f32,
    pub ch2_input_width: f32,
    pub ch2_input_height: f32,

    // Channel 1
    pub ch1_aspect: f32,
    pub ch1_crib_x: f32,
    pub ch1_scale: f32,
    pub ch1_hd_zcrib: f32,
    pub ch1_xy_displace: [f32; 2],
    pub ch1_z_displace: f32,
    pub ch1_rotate: f32,
    pub ch1_hsb_attenuate: Align16,
    pub ch1_posterize: f32,
    pub ch1_posterize_inv: f32,
    pub ch1_kaleidoscope: f32,
    pub ch1_kaleidoscope_slice: f32,
    pub ch1_blur_amount: f32,
    pub ch1_blur_radius: f32,
    pub ch1_sharpen_amount: f32,
    pub ch1_sharpen_radius: f32,
    pub ch1_filters_boost: f32,
    pub ch1_switches: u32,
    pub ch1_geo_overflow: i32,
    pub ch1_hd_aspect_on: i32,
    pub _pad1: f32,

    // Channel 2 mix
    pub ch2_mix_amount: f32,
    _pad_ch2_key: [f32; 2],
    pub ch2_key_value: Align16,
    pub ch2_key_threshold: f32,
    pub ch2_key_soft: f32,
    pub ch2_mix_type: i32,
    pub ch2_mix_overflow: i32,
    pub ch2_key_order: i32,
    pub ch2_key_mode: i32,

    // Channel 2 adjust
    pub ch2_aspect: f32,
    pub ch2_crib_x: f32,
    pub ch2_scale: f32,
    pub ch2_hd_zcrib: f32,
    pub ch2_xy_displace: [f32; 2],
    pub ch2_z_displace: f32,
    pub ch2_rotate: f32,
    _pad_ch2_hsb: [f32; 2],
    pub ch2_hsb_attenuate: Align16,
    pub ch2_posterize: f32,
    pub ch2_posterize_inv: f32,
    pub ch2_kaleidoscope: f32,
    pub ch2_kaleidoscope_slice: f32,
    pub ch2_blur_amount: f32,
    pub ch2_blur_radius: f32,
    pub ch2_sharpen_amount: f32,
    pub ch2_sharpen_radius: f32,
    pub ch2_filters_boost: f32,
    pub ch2_switches: u32,
    pub ch2_geo_overflow: i32,
    pub ch2_hd_aspect_on: i32,
    pub _pad3: f32,

    // FB1 mix
    pub fb1_mix_amount: f32,
    _pad_fb1_key: [f32; 2],
    pub fb1_key_value: Align16,
    pub fb1_key_threshold: f32,
    pub fb1_key_soft: f32,
    pub fb1_mix_type: i32,
    pub fb1_mix_overflow: i32,
    pub fb1_key_order: i32,
    pub _pad4: i32,

    // FB1 geometry
    pub fb1_xy_displace: [f32; 2],
    pub fb1_z_displace: f32,
    pub fb1_rotate: f32,
    _pad_fb1_shear: [f32; 2],
    pub fb1_shear_matrix: [f32; 4],
    pub fb1_kaleidoscope: f32,
    pub fb1_kaleidoscope_slice: f32,
    _pad_fb1_hsb_off: [f32; 2],
    pub fb1_hsb_offset: Align16,

    // FB1 color
    pub fb1_hue_shaper: f32,
    _pad_fb1_hsb_att: [f32; 3],
    pub fb1_hsb_attenuate: Align16,
    pub fb1_hsb_powmap: Align16,
    pub fb1_posterize: f32,
    pub fb1_posterize_inv: f32,

    // FB1 filters
    pub fb1_blur_amount: f32,
    pub fb1_blur_radius: f32,
    pub fb1_sharpen_amount: f32,
    pub fb1_sharpen_radius: f32,
    pub fb1_temporal1_amount: f32,
    pub fb1_temporal1_res: f32,
    pub fb1_temporal2_amount: f32,
    pub fb1_temporal2_res: f32,
    pub fb1_filters_boost: f32,
    pub fb1_switches: u32,
    pub fb1_rotate_mode: i32,
    pub fb1_geo_overflow: i32,
    pub _pad5: f32,

    // Input selection
    pub ch1_input_select: i32,
    pub ch2_input_select: i32,
    pub _pad6: [f32; 3],
}

// ---------------------------------------------------------------------------
// Block B – matches original Block2Uniforms layout (352 bytes)
// ---------------------------------------------------------------------------
#[repr(C, align(16))]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BlockBUniforms {
    pub width: f32,
    pub height: f32,
    pub inv_width: f32,
    pub inv_height: f32,

    // Block 2 input
    pub input_aspect: f32,
    pub input_crib_x: f32,
    pub input_scale: f32,
    pub input_hd_zcrib: f32,
    pub input_xy_displace: [f32; 2],
    pub input_z_displace: f32,
    pub input_rotate: f32,
    pub input_hsb_attenuate: Align16,
    pub input_posterize: f32,
    pub input_posterize_inv: f32,
    pub input_kaleidoscope: f32,
    pub input_kaleidoscope_slice: f32,
    pub input_blur_amount: f32,
    pub input_blur_radius: f32,
    pub input_sharpen_amount: f32,
    pub input_sharpen_radius: f32,
    pub input_filters_boost: f32,
    pub input_switches: u32,
    pub input_posterize_switch: i32,
    pub input_solarize: i32,
    pub input_geo_overflow: i32,
    pub input_hd_aspect_on: i32,
    pub _pad1: f32,

    // FB2
    pub fb2_mix_amount: f32,
    pub fb2_key_value: Align16,
    pub fb2_key_threshold: f32,
    pub fb2_key_soft: f32,
    pub fb2_mix_type: i32,
    pub fb2_mix_overflow: i32,
    pub fb2_key_order: i32,
    pub _pad2: f32,

    pub fb2_xy_displace: [f32; 2],
    pub fb2_z_displace: f32,
    pub fb2_rotate: f32,
    _pad_fb2_shear: [f32; 2],
    pub fb2_shear_matrix: [f32; 4],
    pub fb2_kaleidoscope: f32,
    pub fb2_kaleidoscope_slice: f32,
    _pad_fb2_hsb_off: [f32; 2],
    pub fb2_hsb_offset: Align16,
    pub fb2_hsb_attenuate: Align16,
    pub fb2_hsb_powmap: Align16,
    pub fb2_hue_shaper: f32,
    pub fb2_posterize: f32,
    pub fb2_posterize_inv: f32,
    pub fb2_blur_amount: f32,
    pub fb2_blur_radius: f32,
    pub fb2_sharpen_amount: f32,
    pub fb2_sharpen_radius: f32,
    pub fb2_temporal1_amount: f32,
    pub fb2_temporal1_res: f32,
    pub fb2_temporal2_amount: f32,
    pub fb2_temporal2_res: f32,
    pub fb2_filters_boost: f32,
    pub fb2_switches: u32,
    pub fb2_posterize_switch: i32,
    pub fb2_rotate_mode: i32,
    pub fb2_geo_overflow: i32,

    // Input selection
    pub block2_input_select: i32,
    pub _pad4: [f32; 3],
}

// ---------------------------------------------------------------------------
// Block C – 464 bytes; Align16 = vec4<f32> in WGSL (both are 16 bytes)
// ---------------------------------------------------------------------------
#[repr(C, align(16))]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BlockCUniforms {
    pub width: f32,
    pub height: f32,
    pub inv_width: f32,
    pub inv_height: f32,

    // Block 1 re-process
    pub block1_xy_displace: [f32; 2],
    pub block1_z_displace: f32,
    pub block1_rotate: f32,
    pub block1_shear_matrix: [f32; 4],
    pub block1_kaleidoscope: f32,
    pub block1_kaleidoscope_slice: f32,
    pub block1_blur_amount: f32,
    pub block1_blur_radius: f32,
    pub block1_sharpen_amount: f32,
    pub block1_sharpen_radius: f32,
    pub block1_filters_boost: f32,
    pub block1_dither: f32,
    pub block1_switches: u32,
    pub block1_colorize_mode: i32,
    pub block1_dither_type: i32,
    pub _pad1: f32,

    pub block1_colorize_band1: Align16,
    pub block1_colorize_band2: Align16,
    pub block1_colorize_band3: Align16,
    pub block1_colorize_band4: Align16,
    pub block1_colorize_band5: Align16,

    // Block 2 re-process  (offset 176)
    pub block2_xy_displace: [f32; 2],
    pub block2_z_displace: f32,
    pub block2_rotate: f32,
    pub block2_shear_matrix: [f32; 4],
    pub block2_kaleidoscope: f32,
    pub block2_kaleidoscope_slice: f32,
    pub block2_blur_amount: f32,
    pub block2_blur_radius: f32,
    pub block2_sharpen_amount: f32,
    pub block2_sharpen_radius: f32,
    pub block2_filters_boost: f32,
    pub block2_dither: f32,
    pub block2_switches: u32,
    pub block2_colorize_mode: i32,
    pub block2_dither_type: i32,
    pub _pad7: f32,

    pub block2_colorize_band1: Align16,
    pub block2_colorize_band2: Align16,
    pub block2_colorize_band3: Align16,
    pub block2_colorize_band4: Align16,
    pub block2_colorize_band5: Align16,

    // Matrix mixer  (offset 336)
    pub matrix_mix_type: i32,
    pub matrix_mix_overflow: i32,
    pub _pad13: [f32; 2],          // pad to align next Align16 to offset 352
    pub bg_into_fg_red: Align16,   // offset 352: [r_to_r, g_to_r, b_to_r, _]
    pub bg_into_fg_green: Align16, // offset 368: [r_to_g, g_to_g, b_to_g, _]
    pub bg_into_fg_blue: Align16,  // offset 384: [r_to_b, g_to_b, b_to_b, _]

    // Final mix  (offset 400)
    pub final_mix_amount: f32,
    pub _pad_final_key: [f32; 3],  // pad to align next Align16 to offset 416
    pub final_key_value: Align16,  // offset 416
    pub final_key_threshold: f32,
    pub final_key_soft: f32,
    pub final_mix_type: i32,
    pub final_mix_overflow: i32,
    pub final_key_order: i32,
    pub final_dither: f32,
    pub final_dither_type: i32,
    pub _pad17: f32,               // tail pad to reach 464 bytes total
}

// ---------------------------------------------------------------------------
// Mega-struct
// ---------------------------------------------------------------------------
#[repr(C, align(16))]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
#[derive(Default)]
pub struct WaaavesUniforms {
    pub block_a: BlockAUniforms,
    pub block_b: BlockBUniforms,
    pub block_c: BlockCUniforms,
}

const _: () = assert!(std::mem::size_of::<BlockAUniforms>() == 544);
const _: () = assert!(std::mem::size_of::<BlockBUniforms>() == 352);
const _: () = assert!(std::mem::size_of::<BlockCUniforms>() == 464);
const _: () = assert!(std::mem::size_of::<WaaavesUniforms>().is_multiple_of(16));

impl WaaavesUniforms {
    pub fn from_state(state: &WaaavesState, engine: &EngineState) -> Self {
        Self {
            block_a: build_block_a(state, engine),
            block_b: build_block_b(state, engine),
            block_c: build_block_c(state, engine),
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: read modulated parameter (floats only)
// ---------------------------------------------------------------------------
fn param(id: &str, base: f32, engine: &EngineState) -> f32 {
    engine.get_param(id).unwrap_or(base)
}

// ---------------------------------------------------------------------------
// Switch packing (bit order matches original block1.rs)
// ---------------------------------------------------------------------------
#[allow(clippy::too_many_arguments)]
fn pack_switches(
    h_mirror: bool,
    v_mirror: bool,
    h_flip: bool,
    v_flip: bool,
    hue_inv: bool,
    sat_inv: bool,
    bright_inv: bool,
    rgb_inv: bool,
    solarize: bool,
    posterize: bool,
) -> u32 {
    let mut result = 0u32;
    if h_mirror { result |= 1 << 0; }
    if v_mirror { result |= 1 << 1; }
    if h_flip { result |= 1 << 2; }
    if v_flip { result |= 1 << 3; }
    if hue_inv { result |= 1 << 4; }
    if sat_inv { result |= 1 << 5; }
    if bright_inv { result |= 1 << 6; }
    if rgb_inv { result |= 1 << 7; }
    if solarize { result |= 1 << 8; }
    if posterize { result |= 1 << 9; }
    result
}

// ---------------------------------------------------------------------------
// Block A builder
// ---------------------------------------------------------------------------
fn build_block_a(state: &WaaavesState, engine: &EngineState) -> BlockAUniforms {
    let p = &state.block1;
    let w = engine.resolution.internal_width as f32;
    let h = engine.resolution.internal_height as f32;

    BlockAUniforms {
        width: w,
        height: h,
        inv_width: 1.0 / w.max(1.0),
        inv_height: 1.0 / h.max(1.0),

        ch1_input_width: engine.resolution.input_width as f32,
        ch1_input_height: engine.resolution.input_height as f32,
        ch2_input_width: engine.second_input.width as f32,
        ch2_input_height: engine.second_input.height as f32,

        // Channel 1
        ch1_aspect: 1.0,
        ch1_crib_x: 0.0,
        ch1_scale: 1.0,
        ch1_hd_zcrib: 0.0,
        ch1_xy_displace: [
            param("ch1_x_displace", p.ch1_x_displace, engine),
            param("ch1_y_displace", p.ch1_y_displace, engine),
        ],
        ch1_z_displace: param("ch1_z_displace", p.ch1_z_displace, engine),
        ch1_rotate: param("ch1_rotate", p.ch1_rotate, engine),
        ch1_hsb_attenuate: Align16::new(
            param("ch1_hsb_attenuate_h", p.ch1_hsb_attenuate_h, engine),
            param("ch1_hsb_attenuate_s", p.ch1_hsb_attenuate_s, engine),
            param("ch1_hsb_attenuate_b", p.ch1_hsb_attenuate_b, engine),
        ),
        ch1_posterize: param("ch1_posterize", p.ch1_posterize, engine),
        ch1_posterize_inv: 1.0 / param("ch1_posterize", p.ch1_posterize, engine).max(1.0),
        ch1_kaleidoscope: param("ch1_kaleidoscope_amount", p.ch1_kaleidoscope_amount, engine),
        ch1_kaleidoscope_slice: param("ch1_kaleidoscope_slice", p.ch1_kaleidoscope_slice, engine),
        ch1_blur_amount: param("ch1_blur_amount", p.ch1_blur_amount, engine),
        ch1_blur_radius: param("ch1_blur_radius", p.ch1_blur_radius, engine),
        ch1_sharpen_amount: param("ch1_sharpen_amount", p.ch1_sharpen_amount, engine),
        ch1_sharpen_radius: param("ch1_sharpen_radius", p.ch1_sharpen_radius, engine),
        ch1_filters_boost: param("ch1_filters_boost", p.ch1_filters_boost, engine),
        ch1_switches: pack_switches(
            p.ch1_h_mirror,
            p.ch1_v_mirror,
            p.ch1_h_flip,
            p.ch1_v_flip,
            p.ch1_hue_invert,
            p.ch1_saturation_invert,
            p.ch1_bright_invert,
            p.ch1_rgb_invert,
            p.ch1_solarize,
            p.ch1_posterize_switch,
        ),
        ch1_geo_overflow: p.ch1_geo_overflow,
        ch1_hd_aspect_on: if p.ch1_hd_aspect_on { 1 } else { 0 },
        _pad1: 0.0,

        // Channel 2 mix
        ch2_mix_amount: param("ch2_mix_amount", p.ch2_mix_amount, engine),
        _pad_ch2_key: [0.0; 2],
        ch2_key_value: Align16::new(
            param("ch2_key_value_r", p.ch2_key_value_r, engine),
            param("ch2_key_value_g", p.ch2_key_value_g, engine),
            param("ch2_key_value_b", p.ch2_key_value_b, engine),
        ),
        ch2_key_threshold: param("ch2_key_threshold", p.ch2_key_threshold, engine),
        ch2_key_soft: param("ch2_key_soft", p.ch2_key_soft, engine),
        ch2_mix_type: p.ch2_mix_type,
        ch2_mix_overflow: p.ch2_mix_overflow,
        ch2_key_order: p.ch2_key_order,
        ch2_key_mode: p.ch2_key_mode,

        // Channel 2 adjust
        ch2_aspect: 1.0,
        ch2_crib_x: 0.0,
        ch2_scale: 1.0,
        ch2_hd_zcrib: 0.0,
        ch2_xy_displace: [
            param("ch2_x_displace", p.ch2_x_displace, engine),
            param("ch2_y_displace", p.ch2_y_displace, engine),
        ],
        ch2_z_displace: param("ch2_z_displace", p.ch2_z_displace, engine),
        ch2_rotate: param("ch2_rotate", p.ch2_rotate, engine),
        _pad_ch2_hsb: [0.0; 2],
        ch2_hsb_attenuate: Align16::new(
            param("ch2_hsb_attenuate_h", p.ch2_hsb_attenuate_h, engine),
            param("ch2_hsb_attenuate_s", p.ch2_hsb_attenuate_s, engine),
            param("ch2_hsb_attenuate_b", p.ch2_hsb_attenuate_b, engine),
        ),
        ch2_posterize: param("ch2_posterize", p.ch2_posterize, engine),
        ch2_posterize_inv: 1.0 / param("ch2_posterize", p.ch2_posterize, engine).max(1.0),
        ch2_kaleidoscope: param("ch2_kaleidoscope_amount", p.ch2_kaleidoscope_amount, engine),
        ch2_kaleidoscope_slice: param("ch2_kaleidoscope_slice", p.ch2_kaleidoscope_slice, engine),
        ch2_blur_amount: param("ch2_blur_amount", p.ch2_blur_amount, engine),
        ch2_blur_radius: param("ch2_blur_radius", p.ch2_blur_radius, engine),
        ch2_sharpen_amount: param("ch2_sharpen_amount", p.ch2_sharpen_amount, engine),
        ch2_sharpen_radius: param("ch2_sharpen_radius", p.ch2_sharpen_radius, engine),
        ch2_filters_boost: param("ch2_filters_boost", p.ch2_filters_boost, engine),
        ch2_switches: pack_switches(
            p.ch2_h_mirror,
            p.ch2_v_mirror,
            p.ch2_h_flip,
            p.ch2_v_flip,
            p.ch2_hue_invert,
            p.ch2_saturation_invert,
            p.ch2_bright_invert,
            p.ch2_rgb_invert,
            p.ch2_solarize,
            p.ch2_posterize_switch,
        ),
        ch2_geo_overflow: p.ch2_geo_overflow,
        ch2_hd_aspect_on: if p.ch2_hd_aspect_on { 1 } else { 0 },
        _pad3: 0.0,

        // FB1 mix
        fb1_mix_amount: param("fb1_mix_amount", p.fb1_mix_amount, engine),
        _pad_fb1_key: [0.0; 2],
        fb1_key_value: Align16::new(
            param("fb1_key_value_r", p.fb1_key_value_r, engine),
            param("fb1_key_value_g", p.fb1_key_value_g, engine),
            param("fb1_key_value_b", p.fb1_key_value_b, engine),
        ),
        fb1_key_threshold: param("fb1_key_threshold", p.fb1_key_threshold, engine),
        fb1_key_soft: param("fb1_key_soft", p.fb1_key_soft, engine),
        fb1_mix_type: p.fb1_mix_type,
        fb1_mix_overflow: p.fb1_mix_overflow,
        fb1_key_order: p.fb1_key_order,
        _pad4: 0,

        // FB1 geometry
        fb1_xy_displace: [
            param("fb1_x_displace", p.fb1_x_displace, engine),
            param("fb1_y_displace", p.fb1_y_displace, engine),
        ],
        fb1_z_displace: param("fb1_z_displace", p.fb1_z_displace, engine),
        fb1_rotate: param("fb1_rotate", p.fb1_rotate, engine),
        _pad_fb1_shear: [0.0; 2],
        fb1_shear_matrix: [
            param("fb1_shear_xx", p.fb1_shear_xx, engine),
            param("fb1_shear_xy", p.fb1_shear_xy, engine),
            param("fb1_shear_yx", p.fb1_shear_yx, engine),
            param("fb1_shear_yy", p.fb1_shear_yy, engine),
        ],
        fb1_kaleidoscope: param("fb1_kaleidoscope_amount", p.fb1_kaleidoscope_amount, engine),
        fb1_kaleidoscope_slice: param("fb1_kaleidoscope_slice", p.fb1_kaleidoscope_slice, engine),
        _pad_fb1_hsb_off: [0.0; 2],
        fb1_hsb_offset: Align16::new(
            param("fb1_hsb_offset_h", p.fb1_hsb_offset_h, engine),
            param("fb1_hsb_offset_s", p.fb1_hsb_offset_s, engine),
            param("fb1_hsb_offset_b", p.fb1_hsb_offset_b, engine),
        ),

        // FB1 color
        fb1_hue_shaper: param("fb1_hue_shaper", p.fb1_hue_shaper, engine),
        _pad_fb1_hsb_att: [0.0; 3],
        fb1_hsb_attenuate: Align16::new(
            param("fb1_hsb_attenuate_h", p.fb1_hsb_attenuate_h, engine),
            param("fb1_hsb_attenuate_s", p.fb1_hsb_attenuate_s, engine),
            param("fb1_hsb_attenuate_b", p.fb1_hsb_attenuate_b, engine),
        ),
        fb1_hsb_powmap: Align16::new(
            param("fb1_hsb_powmap_h", p.fb1_hsb_powmap_h, engine),
            param("fb1_hsb_powmap_s", p.fb1_hsb_powmap_s, engine),
            param("fb1_hsb_powmap_b", p.fb1_hsb_powmap_b, engine),
        ),
        fb1_posterize: param("fb1_posterize", p.fb1_posterize, engine),
        fb1_posterize_inv: 1.0 / param("fb1_posterize", p.fb1_posterize, engine).max(1.0),

        // FB1 filters
        fb1_blur_amount: param("fb1_blur_amount", p.fb1_blur_amount, engine),
        fb1_blur_radius: param("fb1_blur_radius", p.fb1_blur_radius, engine),
        fb1_sharpen_amount: param("fb1_sharpen_amount", p.fb1_sharpen_amount, engine),
        fb1_sharpen_radius: param("fb1_sharpen_radius", p.fb1_sharpen_radius, engine),
        fb1_temporal1_amount: param("fb1_temporal1_amount", p.fb1_temporal1_amount, engine),
        fb1_temporal1_res: param("fb1_temporal1_resonance", p.fb1_temporal1_resonance, engine),
        fb1_temporal2_amount: param("fb1_temporal2_amount", p.fb1_temporal2_amount, engine),
        fb1_temporal2_res: param("fb1_temporal2_resonance", p.fb1_temporal2_resonance, engine),
        fb1_filters_boost: param("fb1_filters_boost", p.fb1_filters_boost, engine),
        fb1_switches: pack_switches(
            p.fb1_h_mirror,
            p.fb1_v_mirror,
            p.fb1_h_flip,
            p.fb1_v_flip,
            p.fb1_hue_invert,
            p.fb1_saturation_invert,
            p.fb1_bright_invert,
            false, // rgb_inv placeholder
            false, // solarize placeholder
            p.fb1_posterize_switch,
        ),
        fb1_rotate_mode: p.fb1_rotate_mode,
        fb1_geo_overflow: p.fb1_geo_overflow,
        _pad5: 0.0,

        // Input selection
        ch1_input_select: p.ch1_input_select,
        ch2_input_select: p.ch2_input_select,
        _pad6: [0.0; 3],
    }
}

// ---------------------------------------------------------------------------
// Block B builder
// ---------------------------------------------------------------------------
fn build_block_b(state: &WaaavesState, engine: &EngineState) -> BlockBUniforms {
    let p = &state.block2;
    let w = engine.resolution.internal_width as f32;
    let h = engine.resolution.internal_height as f32;

    let input_switches = pack_switches(
        p.block2_input_h_mirror,
        p.block2_input_v_mirror,
        p.block2_input_h_flip,
        p.block2_input_v_flip,
        p.block2_input_hue_invert,
        p.block2_input_saturation_invert,
        p.block2_input_bright_invert,
        p.block2_input_rgb_invert,
        p.block2_input_solarize,
        p.block2_input_posterize_switch,
    );

    let fb2_switches = pack_switches(
        p.fb2_h_mirror,
        p.fb2_v_mirror,
        p.fb2_h_flip,
        p.fb2_v_flip,
        p.fb2_hue_invert,
        p.fb2_saturation_invert,
        p.fb2_bright_invert,
        p.fb2_rgb_invert,
        false, // solarize placeholder
        p.fb2_posterize_switch,
    );

    BlockBUniforms {
        width: w,
        height: h,
        inv_width: 1.0 / w.max(1.0),
        inv_height: 1.0 / h.max(1.0),

        input_aspect: 1.0,
        input_crib_x: 0.0,
        input_scale: 1.0,
        input_hd_zcrib: 0.0,
        input_xy_displace: [
            param("block2_input_x_displace", p.block2_input_x_displace, engine),
            param("block2_input_y_displace", p.block2_input_y_displace, engine),
        ],
        input_z_displace: param("block2_input_z_displace", p.block2_input_z_displace, engine),
        input_rotate: param("block2_input_rotate", p.block2_input_rotate, engine),
        input_hsb_attenuate: Align16::new(
            param("block2_input_hsb_attenuate_h", p.block2_input_hsb_attenuate_h, engine),
            param("block2_input_hsb_attenuate_s", p.block2_input_hsb_attenuate_s, engine),
            param("block2_input_hsb_attenuate_b", p.block2_input_hsb_attenuate_b, engine),
        ),
        input_posterize: param("block2_input_posterize", p.block2_input_posterize, engine),
        input_posterize_inv: 1.0 / param("block2_input_posterize", p.block2_input_posterize, engine).max(1.0),
        input_kaleidoscope: param("block2_input_kaleidoscope_amount", p.block2_input_kaleidoscope_amount, engine),
        input_kaleidoscope_slice: param("block2_input_kaleidoscope_slice", p.block2_input_kaleidoscope_slice, engine),
        input_blur_amount: param("block2_input_blur_amount", p.block2_input_blur_amount, engine),
        input_blur_radius: param("block2_input_blur_radius", p.block2_input_blur_radius, engine),
        input_sharpen_amount: param("block2_input_sharpen_amount", p.block2_input_sharpen_amount, engine),
        input_sharpen_radius: param("block2_input_sharpen_radius", p.block2_input_sharpen_radius, engine),
        input_filters_boost: param("block2_input_filters_boost", p.block2_input_filters_boost, engine),
        input_switches,
        input_posterize_switch: if p.block2_input_posterize_switch { 1 } else { 0 },
        input_solarize: if p.block2_input_solarize { 1 } else { 0 },
        input_geo_overflow: p.block2_input_geo_overflow,
        input_hd_aspect_on: if p.block2_input_hd_aspect_on { 1 } else { 0 },
        _pad1: 0.0,

        fb2_mix_amount: param("fb2_mix_amount", p.fb2_mix_amount, engine),
        fb2_key_value: Align16::new(
            param("fb2_key_value_r", p.fb2_key_value_r, engine),
            param("fb2_key_value_g", p.fb2_key_value_g, engine),
            param("fb2_key_value_b", p.fb2_key_value_b, engine),
        ),
        fb2_key_threshold: param("fb2_key_threshold", p.fb2_key_threshold, engine),
        fb2_key_soft: param("fb2_key_soft", p.fb2_key_soft, engine),
        fb2_mix_type: p.fb2_mix_type,
        fb2_mix_overflow: p.fb2_mix_overflow,
        fb2_key_order: p.fb2_key_order,
        _pad2: 0.0,

        fb2_xy_displace: [
            param("fb2_x_displace", p.fb2_x_displace, engine),
            param("fb2_y_displace", p.fb2_y_displace, engine),
        ],
        fb2_z_displace: param("fb2_z_displace", p.fb2_z_displace, engine),
        fb2_rotate: param("fb2_rotate", p.fb2_rotate, engine),
        _pad_fb2_shear: [0.0; 2],
        fb2_shear_matrix: [
            param("fb2_shear_xx", p.fb2_shear_xx, engine),
            param("fb2_shear_xy", p.fb2_shear_xy, engine),
            param("fb2_shear_yx", p.fb2_shear_yx, engine),
            param("fb2_shear_yy", p.fb2_shear_yy, engine),
        ],
        fb2_kaleidoscope: param("fb2_kaleidoscope_amount", p.fb2_kaleidoscope_amount, engine),
        fb2_kaleidoscope_slice: param("fb2_kaleidoscope_slice", p.fb2_kaleidoscope_slice, engine),
        _pad_fb2_hsb_off: [0.0; 2],
        fb2_hsb_offset: Align16::new(
            param("fb2_hsb_offset_h", p.fb2_hsb_offset_h, engine),
            param("fb2_hsb_offset_s", p.fb2_hsb_offset_s, engine),
            param("fb2_hsb_offset_b", p.fb2_hsb_offset_b, engine),
        ),
        fb2_hsb_attenuate: Align16::new(
            param("fb2_hsb_attenuate_h", p.fb2_hsb_attenuate_h, engine),
            param("fb2_hsb_attenuate_s", p.fb2_hsb_attenuate_s, engine),
            param("fb2_hsb_attenuate_b", p.fb2_hsb_attenuate_b, engine),
        ),
        fb2_hsb_powmap: Align16::new(
            param("fb2_hsb_powmap_h", p.fb2_hsb_powmap_h, engine),
            param("fb2_hsb_powmap_s", p.fb2_hsb_powmap_s, engine),
            param("fb2_hsb_powmap_b", p.fb2_hsb_powmap_b, engine),
        ),
        fb2_hue_shaper: param("fb2_hue_shaper", p.fb2_hue_shaper, engine),
        fb2_posterize: param("fb2_posterize", p.fb2_posterize, engine),
        fb2_posterize_inv: 1.0 / param("fb2_posterize", p.fb2_posterize, engine).max(1.0),
        fb2_blur_amount: param("fb2_blur_amount", p.fb2_blur_amount, engine),
        fb2_blur_radius: param("fb2_blur_radius", p.fb2_blur_radius, engine),
        fb2_sharpen_amount: param("fb2_sharpen_amount", p.fb2_sharpen_amount, engine),
        fb2_sharpen_radius: param("fb2_sharpen_radius", p.fb2_sharpen_radius, engine),
        fb2_temporal1_amount: param("fb2_temporal1_amount", p.fb2_temporal1_amount, engine),
        fb2_temporal1_res: param("fb2_temporal1_resonance", p.fb2_temporal1_resonance, engine),
        fb2_temporal2_amount: param("fb2_temporal2_amount", p.fb2_temporal2_amount, engine),
        fb2_temporal2_res: param("fb2_temporal2_resonance", p.fb2_temporal2_resonance, engine),
        fb2_filters_boost: param("fb2_filters_boost", p.fb2_filters_boost, engine),
        fb2_switches,
        fb2_posterize_switch: if p.fb2_posterize_switch { 1 } else { 0 },
        fb2_rotate_mode: p.fb2_rotate_mode,
        fb2_geo_overflow: p.fb2_geo_overflow,

        block2_input_select: p.block2_input_select,
        _pad4: [0.0; 3],
    }
}

// ---------------------------------------------------------------------------
// Block C builder
// ---------------------------------------------------------------------------
fn build_block_c(state: &WaaavesState, engine: &EngineState) -> BlockCUniforms {
    let p = &state.block3;
    let w = engine.resolution.internal_width as f32;
    let h = engine.resolution.internal_height as f32;

    BlockCUniforms {
        width: w,
        height: h,
        inv_width: 1.0 / w.max(1.0),
        inv_height: 1.0 / h.max(1.0),

        block1_xy_displace: [
            param("block1_x_displace", p.block1_x_displace, engine),
            param("block1_y_displace", p.block1_y_displace, engine),
        ],
        block1_z_displace: param("block1_z_displace", p.block1_z_displace, engine),
        block1_rotate: param("block1_rotate", p.block1_rotate, engine),
        block1_shear_matrix: [
            param("block1_shear_xx", p.block1_shear_xx, engine),
            param("block1_shear_xy", p.block1_shear_xy, engine),
            param("block1_shear_yx", p.block1_shear_yx, engine),
            param("block1_shear_yy", p.block1_shear_yy, engine),
        ],
        block1_kaleidoscope: param("block1_kaleidoscope_amount", p.block1_kaleidoscope_amount, engine),
        block1_kaleidoscope_slice: param("block1_kaleidoscope_slice", p.block1_kaleidoscope_slice, engine),
        block1_blur_amount: param("block1_blur_amount", p.block1_blur_amount, engine),
        block1_blur_radius: param("block1_blur_radius", p.block1_blur_radius, engine),
        block1_sharpen_amount: param("block1_sharpen_amount", p.block1_sharpen_amount, engine),
        block1_sharpen_radius: param("block1_sharpen_radius", p.block1_sharpen_radius, engine),
        block1_filters_boost: param("block1_filters_boost", p.block1_filters_boost, engine),
        block1_dither: if p.block1_dither_switch {
            param("block1_dither", p.block1_dither, engine).max(1.0)
        } else {
            0.0
        },
        block1_switches: if p.block1_colorize_switch { 1 } else { 0 },
        block1_colorize_mode: p.block1_colorize_hsb_rgb,
        block1_dither_type: p.block1_dither_type,
        _pad1: 0.0,

        block1_colorize_band1: Align16::new(p.block1_colorize_band1_h, p.block1_colorize_band1_s, p.block1_colorize_band1_b),
        block1_colorize_band2: Align16::new(p.block1_colorize_band2_h, p.block1_colorize_band2_s, p.block1_colorize_band2_b),
        block1_colorize_band3: Align16::new(p.block1_colorize_band3_h, p.block1_colorize_band3_s, p.block1_colorize_band3_b),
        block1_colorize_band4: Align16::new(p.block1_colorize_band4_h, p.block1_colorize_band4_s, p.block1_colorize_band4_b),
        block1_colorize_band5: Align16::new(p.block1_colorize_band5_h, p.block1_colorize_band5_s, p.block1_colorize_band5_b),

        block2_xy_displace: [
            param("block2_x_displace", p.block2_x_displace, engine),
            param("block2_y_displace", p.block2_y_displace, engine),
        ],
        block2_z_displace: param("block2_z_displace", p.block2_z_displace, engine),
        block2_rotate: param("block2_rotate", p.block2_rotate, engine),
        block2_shear_matrix: [
            param("block2_shear_xx", p.block2_shear_xx, engine),
            param("block2_shear_xy", p.block2_shear_xy, engine),
            param("block2_shear_yx", p.block2_shear_yx, engine),
            param("block2_shear_yy", p.block2_shear_yy, engine),
        ],
        block2_kaleidoscope: param("block2_kaleidoscope_amount", p.block2_kaleidoscope_amount, engine),
        block2_kaleidoscope_slice: param("block2_kaleidoscope_slice", p.block2_kaleidoscope_slice, engine),
        block2_blur_amount: param("block2_blur_amount", p.block2_blur_amount, engine),
        block2_blur_radius: param("block2_blur_radius", p.block2_blur_radius, engine),
        block2_sharpen_amount: param("block2_sharpen_amount", p.block2_sharpen_amount, engine),
        block2_sharpen_radius: param("block2_sharpen_radius", p.block2_sharpen_radius, engine),
        block2_filters_boost: param("block2_filters_boost", p.block2_filters_boost, engine),
        block2_dither: if p.block2_dither_switch {
            param("block2_dither", p.block2_dither, engine).max(1.0)
        } else {
            0.0
        },
        block2_switches: if p.block2_colorize_switch { 1 } else { 0 },
        block2_colorize_mode: p.block2_colorize_hsb_rgb,
        block2_dither_type: p.block2_dither_type,
        _pad7: 0.0,

        block2_colorize_band1: Align16::new(p.block2_colorize_band1_h, p.block2_colorize_band1_s, p.block2_colorize_band1_b),
        block2_colorize_band2: Align16::new(p.block2_colorize_band2_h, p.block2_colorize_band2_s, p.block2_colorize_band2_b),
        block2_colorize_band3: Align16::new(p.block2_colorize_band3_h, p.block2_colorize_band3_s, p.block2_colorize_band3_b),
        block2_colorize_band4: Align16::new(p.block2_colorize_band4_h, p.block2_colorize_band4_s, p.block2_colorize_band4_b),
        block2_colorize_band5: Align16::new(p.block2_colorize_band5_h, p.block2_colorize_band5_s, p.block2_colorize_band5_b),

        matrix_mix_type: p.matrix_mix_type,
        matrix_mix_overflow: p.matrix_mix_overflow,
        _pad13: [0.0; 2],
        // Columns of the 3×3 matrix: each vec packs "how much each source channel
        // contributes to the destination channel" (column-major from WGSL's perspective).
        // bg_into_fg_red.xyz = [r_to_r, g_to_r, b_to_r]
        bg_into_fg_red: Align16::new(
            param("matrix_mix_r_to_r", p.matrix_mix_r_to_r, engine),
            param("matrix_mix_g_to_r", p.matrix_mix_g_to_r, engine),
            param("matrix_mix_b_to_r", p.matrix_mix_b_to_r, engine),
        ),
        bg_into_fg_green: Align16::new(
            param("matrix_mix_r_to_g", p.matrix_mix_r_to_g, engine),
            param("matrix_mix_g_to_g", p.matrix_mix_g_to_g, engine),
            param("matrix_mix_b_to_g", p.matrix_mix_b_to_g, engine),
        ),
        bg_into_fg_blue: Align16::new(
            param("matrix_mix_r_to_b", p.matrix_mix_r_to_b, engine),
            param("matrix_mix_g_to_b", p.matrix_mix_g_to_b, engine),
            param("matrix_mix_b_to_b", p.matrix_mix_b_to_b, engine),
        ),

        final_mix_amount: param("final_mix_amount", p.final_mix_amount, engine),
        _pad_final_key: [0.0; 3],
        final_key_value: Align16::new(
            param("final_key_value_r", p.final_key_value_r, engine),
            param("final_key_value_g", p.final_key_value_g, engine),
            param("final_key_value_b", p.final_key_value_b, engine),
        ),
        final_key_threshold: param("final_key_threshold", p.final_key_threshold, engine),
        final_key_soft: param("final_key_soft", p.final_key_soft, engine),
        final_mix_type: p.final_mix_type,
        final_mix_overflow: p.final_mix_overflow,
        final_key_order: p.final_key_order,
        final_dither: if p.final_dither_switch {
            param("final_dither", p.final_dither, engine).max(1.0)
        } else {
            0.0
        },
        final_dither_type: p.final_dither_type,
        _pad17: 0.0,
    }
}

impl Default for Align16 {
    fn default() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }
}

impl Default for BlockAUniforms {
    fn default() -> Self {
        Self {
            width: 1280.0,
            height: 720.0,
            inv_width: 1.0 / 1280.0,
            inv_height: 1.0 / 720.0,
            ch1_input_width: 1280.0,
            ch1_input_height: 720.0,
            ch2_input_width: 1280.0,
            ch2_input_height: 720.0,
            ch1_aspect: 1.0,
            ch1_crib_x: 0.0,
            ch1_scale: 1.0,
            ch1_hd_zcrib: 0.0,
            ch1_xy_displace: [0.0; 2],
            ch1_z_displace: 1.0,
            ch1_rotate: 0.0,
            ch1_hsb_attenuate: Align16::new(1.0, 1.0, 1.0),
            ch1_posterize: 16.0,
            ch1_posterize_inv: 1.0 / 16.0,
            ch1_kaleidoscope: 0.0,
            ch1_kaleidoscope_slice: 0.0,
            ch1_blur_amount: 0.0,
            ch1_blur_radius: 1.0,
            ch1_sharpen_amount: 0.0,
            ch1_sharpen_radius: 1.0,
            ch1_filters_boost: 0.0,
            ch1_switches: 0,
            ch1_geo_overflow: 0,
            ch1_hd_aspect_on: 0,
            _pad1: 0.0,
            ch2_mix_amount: 0.0,
            _pad_ch2_key: [0.0; 2],
            ch2_key_value: Align16::new(0.0, 0.0, 0.0),
            ch2_key_threshold: 1.0,
            ch2_key_soft: 0.0,
            ch2_mix_type: 0,
            ch2_mix_overflow: 0,
            ch2_key_order: 0,
            ch2_key_mode: 0,
            ch2_aspect: 1.0,
            ch2_crib_x: 0.0,
            ch2_scale: 1.0,
            ch2_hd_zcrib: 0.0,
            ch2_xy_displace: [0.0; 2],
            ch2_z_displace: 1.0,
            ch2_rotate: 0.0,
            _pad_ch2_hsb: [0.0; 2],
            ch2_hsb_attenuate: Align16::new(1.0, 1.0, 1.0),
            ch2_posterize: 16.0,
            ch2_posterize_inv: 1.0 / 16.0,
            ch2_kaleidoscope: 0.0,
            ch2_kaleidoscope_slice: 0.0,
            ch2_blur_amount: 0.0,
            ch2_blur_radius: 1.0,
            ch2_sharpen_amount: 0.0,
            ch2_sharpen_radius: 1.0,
            ch2_filters_boost: 0.0,
            ch2_switches: 0,
            ch2_geo_overflow: 0,
            ch2_hd_aspect_on: 0,
            _pad3: 0.0,
            fb1_mix_amount: 0.0,
            _pad_fb1_key: [0.0; 2],
            fb1_key_value: Align16::new(0.0, 0.0, 0.0),
            fb1_key_threshold: 1.0,
            fb1_key_soft: 0.0,
            fb1_mix_type: 0,
            fb1_mix_overflow: 0,
            fb1_key_order: 0,
            _pad4: 0,
            fb1_xy_displace: [0.0; 2],
            fb1_z_displace: 1.0,
            fb1_rotate: 0.0,
            _pad_fb1_shear: [0.0; 2],
            fb1_shear_matrix: [1.0, 0.0, 0.0, 1.0],
            fb1_kaleidoscope: 0.0,
            fb1_kaleidoscope_slice: 0.0,
            _pad_fb1_hsb_off: [0.0; 2],
            fb1_hsb_offset: Align16::new(0.0, 0.0, 0.0),
            fb1_hue_shaper: 1.0,
            _pad_fb1_hsb_att: [0.0; 3],
            fb1_hsb_attenuate: Align16::new(1.0, 1.0, 1.0),
            fb1_hsb_powmap: Align16::new(1.0, 1.0, 1.0),
            fb1_posterize: 16.0,
            fb1_posterize_inv: 1.0 / 16.0,
            fb1_blur_amount: 0.0,
            fb1_blur_radius: 1.0,
            fb1_sharpen_amount: 0.0,
            fb1_sharpen_radius: 1.0,
            fb1_temporal1_amount: 0.0,
            fb1_temporal1_res: 0.0,
            fb1_temporal2_amount: 0.0,
            fb1_temporal2_res: 0.0,
            fb1_filters_boost: 0.0,
            fb1_switches: 0,
            fb1_rotate_mode: 0,
            fb1_geo_overflow: 0,
            _pad5: 0.0,
            ch1_input_select: 0,
            ch2_input_select: 0,
            _pad6: [0.0; 3],
        }
    }
}

impl Default for BlockBUniforms {
    fn default() -> Self {
        Self {
            width: 1280.0,
            height: 720.0,
            inv_width: 1.0 / 1280.0,
            inv_height: 1.0 / 720.0,
            input_aspect: 1.0,
            input_crib_x: 0.0,
            input_scale: 1.0,
            input_hd_zcrib: 0.0,
            input_xy_displace: [0.0; 2],
            input_z_displace: 1.0,
            input_rotate: 0.0,
            input_hsb_attenuate: Align16::new(1.0, 1.0, 1.0),
            input_posterize: 16.0,
            input_posterize_inv: 1.0 / 16.0,
            input_kaleidoscope: 0.0,
            input_kaleidoscope_slice: 0.0,
            input_blur_amount: 0.0,
            input_blur_radius: 1.0,
            input_sharpen_amount: 0.0,
            input_sharpen_radius: 1.0,
            input_filters_boost: 0.0,
            input_switches: 0,
            input_posterize_switch: 0,
            input_solarize: 0,
            input_geo_overflow: 0,
            input_hd_aspect_on: 0,
            _pad1: 0.0,
            fb2_mix_amount: 0.0,
            fb2_key_value: Align16::new(0.0, 0.0, 0.0),
            fb2_key_threshold: 1.0,
            fb2_key_soft: 0.0,
            fb2_mix_type: 0,
            fb2_mix_overflow: 0,
            fb2_key_order: 0,
            _pad2: 0.0,
            fb2_xy_displace: [0.0; 2],
            fb2_z_displace: 1.0,
            fb2_rotate: 0.0,
            _pad_fb2_shear: [0.0; 2],
            fb2_shear_matrix: [1.0, 0.0, 0.0, 1.0],
            fb2_kaleidoscope: 0.0,
            fb2_kaleidoscope_slice: 0.0,
            _pad_fb2_hsb_off: [0.0; 2],
            fb2_hsb_offset: Align16::new(0.0, 0.0, 0.0),
            fb2_hsb_attenuate: Align16::new(1.0, 1.0, 1.0),
            fb2_hsb_powmap: Align16::new(1.0, 1.0, 1.0),
            fb2_hue_shaper: 1.0,
            fb2_posterize: 16.0,
            fb2_posterize_inv: 1.0 / 16.0,
            fb2_blur_amount: 0.0,
            fb2_blur_radius: 1.0,
            fb2_sharpen_amount: 0.0,
            fb2_sharpen_radius: 1.0,
            fb2_temporal1_amount: 0.0,
            fb2_temporal1_res: 0.0,
            fb2_temporal2_amount: 0.0,
            fb2_temporal2_res: 0.0,
            fb2_filters_boost: 0.0,
            fb2_switches: 0,
            fb2_posterize_switch: 0,
            fb2_rotate_mode: 0,
            fb2_geo_overflow: 0,
            block2_input_select: 0,
            _pad4: [0.0; 3],
        }
    }
}

impl Default for BlockCUniforms {
    fn default() -> Self {
        Self {
            width: 1280.0,
            height: 720.0,
            inv_width: 1.0 / 1280.0,
            inv_height: 1.0 / 720.0,
            block1_xy_displace: [0.0; 2],
            block1_z_displace: 1.0,
            block1_rotate: 0.0,
            block1_shear_matrix: [1.0, 0.0, 0.0, 1.0],
            block1_kaleidoscope: 0.0,
            block1_kaleidoscope_slice: 0.0,
            block1_blur_amount: 0.0,
            block1_blur_radius: 1.0,
            block1_sharpen_amount: 0.0,
            block1_sharpen_radius: 1.0,
            block1_filters_boost: 0.0,
            block1_dither: 16.0,
            block1_switches: 0,
            block1_colorize_mode: 0,
            block1_dither_type: 0,
            _pad1: 0.0,
            block1_colorize_band1: Align16::new(0.0, 0.0, 0.0),
            block1_colorize_band2: Align16::new(0.0, 0.0, 0.0),
            block1_colorize_band3: Align16::new(0.0, 0.0, 0.0),
            block1_colorize_band4: Align16::new(0.0, 0.0, 0.0),
            block1_colorize_band5: Align16::new(0.0, 0.0, 0.0),
            block2_xy_displace: [0.0; 2],
            block2_z_displace: 1.0,
            block2_rotate: 0.0,
            block2_shear_matrix: [1.0, 0.0, 0.0, 1.0],
            block2_kaleidoscope: 0.0,
            block2_kaleidoscope_slice: 0.0,
            block2_blur_amount: 0.0,
            block2_blur_radius: 1.0,
            block2_sharpen_amount: 0.0,
            block2_sharpen_radius: 1.0,
            block2_filters_boost: 0.0,
            block2_dither: 16.0,
            block2_switches: 0,
            block2_colorize_mode: 0,
            block2_dither_type: 0,
            _pad7: 0.0,
            block2_colorize_band1: Align16::new(0.0, 0.0, 0.0),
            block2_colorize_band2: Align16::new(0.0, 0.0, 0.0),
            block2_colorize_band3: Align16::new(0.0, 0.0, 0.0),
            block2_colorize_band4: Align16::new(0.0, 0.0, 0.0),
            block2_colorize_band5: Align16::new(0.0, 0.0, 0.0),
            matrix_mix_type: 0,
            matrix_mix_overflow: 0,
            _pad13: [0.0; 2],
            bg_into_fg_red: Align16::new(0.0, 0.0, 0.0),
            bg_into_fg_green: Align16::new(0.0, 0.0, 0.0),
            bg_into_fg_blue: Align16::new(0.0, 0.0, 0.0),
            final_mix_amount: 0.0,
            _pad_final_key: [0.0; 3],
            final_key_value: Align16::new(0.0, 0.0, 0.0),
            final_key_threshold: 1.0,
            final_key_soft: 0.0,
            final_mix_type: 0,
            final_mix_overflow: 0,
            final_key_order: 0,
            final_dither: 0.0,
            final_dither_type: 0,
            _pad17: 0.0,
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_is_multiple_of_16() {
        assert_eq!(std::mem::size_of::<WaaavesUniforms>() % 16, 0);
    }

    #[test]
    fn bytes_of_default_succeeds() {
        let u = WaaavesUniforms::default();
        let bytes = bytemuck::bytes_of(&u);
        assert_eq!(bytes.len(), std::mem::size_of::<WaaavesUniforms>());
    }
}
