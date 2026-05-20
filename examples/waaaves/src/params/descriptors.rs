use rustjay_engine::prelude::{ParameterDescriptor, ParamCategory};

macro_rules! f {
    ($id:expr, $name:expr, $cat:expr, $min:expr, $max:expr, $default:expr, $step:expr) => {
        ParameterDescriptor::float($id, $name, $cat, $min, $max, $default, $step)
    };
}

pub fn waaaves_parameter_descriptors() -> Vec<ParameterDescriptor> {
    let b1_ch1 = ParamCategory::Custom("Block 1 — CH1".into());
    let b1_ch2 = ParamCategory::Custom("Block 1 — CH2".into());
    let b1_fb1 = ParamCategory::Custom("Block 1 — FB1".into());
    let b2_in  = ParamCategory::Custom("Block 2 — Input".into());
    let b2_fb2 = ParamCategory::Custom("Block 2 — FB2".into());
    let b3_b1  = ParamCategory::Custom("Block 3 — B1 Re-process".into());
    let b3_b2  = ParamCategory::Custom("Block 3 — B2 Re-process".into());
    let b3_mat = ParamCategory::Custom("Block 3 — Matrix".into());
    let b3_fin = ParamCategory::Custom("Block 3 — Final".into());

    vec![
        // =====================================================================
        // Block 1 — CH1
        // =====================================================================
        f!("ch1_x_displace",          "CH1 X Displace",          b1_ch1.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("ch1_y_displace",          "CH1 Y Displace",          b1_ch1.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("ch1_z_displace",          "CH1 Z Displace",          b1_ch1.clone(),  0.0,  4.0, 1.0, 0.01),
        f!("ch1_rotate",              "CH1 Rotate",              b1_ch1.clone(), -std::f32::consts::TAU, std::f32::consts::TAU, 0.0, 0.01),
        f!("ch1_hsb_attenuate_h",     "CH1 Hue Attenuate",       b1_ch1.clone(),  0.0,  2.0, 1.0, 0.01),
        f!("ch1_hsb_attenuate_s",     "CH1 Sat Attenuate",       b1_ch1.clone(),  0.0,  2.0, 1.0, 0.01),
        f!("ch1_hsb_attenuate_b",     "CH1 Bri Attenuate",       b1_ch1.clone(),  0.0,  2.0, 1.0, 0.01),
        f!("ch1_kaleidoscope_amount", "CH1 Kaleidoscope",        b1_ch1.clone(),  0.0,  1.0, 0.0, 0.01),
        f!("ch1_kaleidoscope_slice",  "CH1 Kaleidoscope Slice",  b1_ch1.clone(), -std::f32::consts::PI, std::f32::consts::PI, 0.0, 0.01),
        f!("ch1_blur_amount",         "CH1 Blur Amount",         b1_ch1.clone(),  0.0,  1.0, 0.0, 0.01),
        f!("ch1_sharpen_amount",      "CH1 Sharpen Amount",      b1_ch1.clone(),  0.0,  1.0, 0.0, 0.01),
        f!("ch1_filters_boost",       "CH1 Filters Boost",       b1_ch1.clone(),  0.0,  1.0, 0.0, 0.01),

        // =====================================================================
        // Block 1 — CH2 Mix
        // =====================================================================
        f!("ch2_mix_amount",   "CH2 Mix Amount",   b1_ch2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("ch2_key_threshold","CH2 Key Threshold",b1_ch2.clone(), 0.0, 1.0, 1.0, 0.01),
        f!("ch2_key_soft",     "CH2 Key Soft",     b1_ch2.clone(), 0.0, 1.0, 0.0, 0.01),

        // =====================================================================
        // Block 1 — CH2 Adjust
        // =====================================================================
        f!("ch2_x_displace",          "CH2 X Displace",          b1_ch2.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("ch2_y_displace",          "CH2 Y Displace",          b1_ch2.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("ch2_z_displace",          "CH2 Z Displace",          b1_ch2.clone(),  0.0,  4.0, 1.0, 0.01),
        f!("ch2_rotate",              "CH2 Rotate",              b1_ch2.clone(), -std::f32::consts::TAU, std::f32::consts::TAU, 0.0, 0.01),
        f!("ch2_hsb_attenuate_h",     "CH2 Hue Attenuate",       b1_ch2.clone(),  0.0,  2.0, 1.0, 0.01),
        f!("ch2_hsb_attenuate_s",     "CH2 Sat Attenuate",       b1_ch2.clone(),  0.0,  2.0, 1.0, 0.01),
        f!("ch2_hsb_attenuate_b",     "CH2 Bri Attenuate",       b1_ch2.clone(),  0.0,  2.0, 1.0, 0.01),
        f!("ch2_kaleidoscope_amount", "CH2 Kaleidoscope",        b1_ch2.clone(),  0.0,  1.0, 0.0, 0.01),
        f!("ch2_kaleidoscope_slice",  "CH2 Kaleidoscope Slice",  b1_ch2.clone(), -std::f32::consts::PI, std::f32::consts::PI, 0.0, 0.01),
        f!("ch2_blur_amount",         "CH2 Blur Amount",         b1_ch2.clone(),  0.0,  1.0, 0.0, 0.01),
        f!("ch2_sharpen_amount",      "CH2 Sharpen Amount",      b1_ch2.clone(),  0.0,  1.0, 0.0, 0.01),
        f!("ch2_filters_boost",       "CH2 Filters Boost",       b1_ch2.clone(),  0.0,  1.0, 0.0, 0.01),

        // =====================================================================
        // Block 1 — FB1 Mix
        // =====================================================================
        f!("fb1_mix_amount",   "FB1 Mix Amount",   b1_fb1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("fb1_key_threshold","FB1 Key Threshold",b1_fb1.clone(), 0.0, 1.0, 1.0, 0.01),
        f!("fb1_key_soft",     "FB1 Key Soft",     b1_fb1.clone(), 0.0, 1.0, 0.0, 0.01),

        // =====================================================================
        // Block 1 — FB1 Geometry
        // =====================================================================
        f!("fb1_x_displace",          "FB1 X Displace",          b1_fb1.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("fb1_y_displace",          "FB1 Y Displace",          b1_fb1.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("fb1_z_displace",          "FB1 Z Displace",          b1_fb1.clone(),  0.0,  4.0, 1.0, 0.01),
        f!("fb1_rotate",              "FB1 Rotate",              b1_fb1.clone(), -std::f32::consts::TAU, std::f32::consts::TAU, 0.0, 0.01),
        f!("fb1_shear_xx",            "FB1 Shear XX",            b1_fb1.clone(), -2.0,  2.0, 1.0, 0.01),
        f!("fb1_shear_xy",            "FB1 Shear XY",            b1_fb1.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("fb1_shear_yx",            "FB1 Shear YX",            b1_fb1.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("fb1_shear_yy",            "FB1 Shear YY",            b1_fb1.clone(), -2.0,  2.0, 1.0, 0.01),
        f!("fb1_kaleidoscope_amount", "FB1 Kaleidoscope",        b1_fb1.clone(),  0.0,  1.0, 0.0, 0.01),
        f!("fb1_kaleidoscope_slice",  "FB1 Kaleidoscope Slice",  b1_fb1.clone(), -std::f32::consts::PI, std::f32::consts::PI, 0.0, 0.01),

        // =====================================================================
        // Block 1 — FB1 Color
        // =====================================================================
        f!("fb1_hsb_offset_h",     "FB1 Hue Offset",     b1_fb1.clone(), -1.0, 1.0, 0.0, 0.01),
        f!("fb1_hsb_offset_s",     "FB1 Sat Offset",     b1_fb1.clone(), -1.0, 1.0, 0.0, 0.01),
        f!("fb1_hsb_offset_b",     "FB1 Bri Offset",     b1_fb1.clone(), -1.0, 1.0, 0.0, 0.01),
        f!("fb1_hsb_attenuate_h",  "FB1 Hue Attenuate",  b1_fb1.clone(),  0.0, 2.0, 1.0, 0.01),
        f!("fb1_hsb_attenuate_s",  "FB1 Sat Attenuate",  b1_fb1.clone(),  0.0, 2.0, 1.0, 0.01),
        f!("fb1_hsb_attenuate_b",  "FB1 Bri Attenuate",  b1_fb1.clone(),  0.0, 2.0, 1.0, 0.01),
        f!("fb1_hsb_powmap_h",     "FB1 Hue PowMap",     b1_fb1.clone(),  0.0, 2.0, 1.0, 0.01),
        f!("fb1_hsb_powmap_s",     "FB1 Sat PowMap",     b1_fb1.clone(),  0.0, 2.0, 1.0, 0.01),
        f!("fb1_hsb_powmap_b",     "FB1 Bri PowMap",     b1_fb1.clone(),  0.0, 2.0, 1.0, 0.01),
        f!("fb1_hue_shaper",       "FB1 Hue Shaper",     b1_fb1.clone(),  0.0, 2.0, 1.0, 0.01),

        // =====================================================================
        // Block 1 — FB1 Filters
        // =====================================================================
        f!("fb1_blur_amount",       "FB1 Blur Amount",       b1_fb1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("fb1_sharpen_amount",    "FB1 Sharpen Amount",    b1_fb1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("fb1_filters_boost",     "FB1 Filters Boost",     b1_fb1.clone(), 0.0, 1.0, 0.0, 0.01),

        // =====================================================================
        // Block 2 — Input
        // =====================================================================
        f!("block2_input_x_displace",          "B2 Input X Displace",          b2_in.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("block2_input_y_displace",          "B2 Input Y Displace",          b2_in.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("block2_input_z_displace",          "B2 Input Z Displace",          b2_in.clone(),  0.0,  4.0, 1.0, 0.01),
        f!("block2_input_rotate",              "B2 Input Rotate",              b2_in.clone(), -std::f32::consts::TAU, std::f32::consts::TAU, 0.0, 0.01),
        f!("block2_input_hsb_attenuate_h",     "B2 Input Hue Attenuate",       b2_in.clone(),  0.0,  2.0, 1.0, 0.01),
        f!("block2_input_hsb_attenuate_s",     "B2 Input Sat Attenuate",       b2_in.clone(),  0.0,  2.0, 1.0, 0.01),
        f!("block2_input_hsb_attenuate_b",     "B2 Input Bri Attenuate",       b2_in.clone(),  0.0,  2.0, 1.0, 0.01),
        f!("block2_input_kaleidoscope_amount", "B2 Input Kaleidoscope",        b2_in.clone(),  0.0,  1.0, 0.0, 0.01),
        f!("block2_input_kaleidoscope_slice",  "B2 Input Kaleidoscope Slice",  b2_in.clone(), -std::f32::consts::PI, std::f32::consts::PI, 0.0, 0.01),
        f!("block2_input_blur_amount",         "B2 Input Blur Amount",         b2_in.clone(),  0.0,  1.0, 0.0, 0.01),
        f!("block2_input_sharpen_amount",      "B2 Input Sharpen Amount",      b2_in.clone(),  0.0,  1.0, 0.0, 0.01),

        // =====================================================================
        // Block 2 — FB2 Mix
        // =====================================================================
        f!("fb2_mix_amount",   "FB2 Mix Amount",   b2_fb2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("fb2_key_threshold","FB2 Key Threshold",b2_fb2.clone(), 0.0, 1.0, 1.0, 0.01),
        f!("fb2_key_soft",     "FB2 Key Soft",     b2_fb2.clone(), 0.0, 1.0, 0.0, 0.01),

        // =====================================================================
        // Block 2 — FB2 Geometry
        // =====================================================================
        f!("fb2_x_displace",          "FB2 X Displace",          b2_fb2.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("fb2_y_displace",          "FB2 Y Displace",          b2_fb2.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("fb2_z_displace",          "FB2 Z Displace",          b2_fb2.clone(),  0.0,  4.0, 1.0, 0.01),
        f!("fb2_rotate",              "FB2 Rotate",              b2_fb2.clone(), -std::f32::consts::TAU, std::f32::consts::TAU, 0.0, 0.01),
        f!("fb2_shear_xx",            "FB2 Shear XX",            b2_fb2.clone(), -2.0,  2.0, 1.0, 0.01),
        f!("fb2_shear_xy",            "FB2 Shear XY",            b2_fb2.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("fb2_shear_yx",            "FB2 Shear YX",            b2_fb2.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("fb2_shear_yy",            "FB2 Shear YY",            b2_fb2.clone(), -2.0,  2.0, 1.0, 0.01),
        f!("fb2_kaleidoscope_amount", "FB2 Kaleidoscope",        b2_fb2.clone(),  0.0,  1.0, 0.0, 0.01),
        f!("fb2_kaleidoscope_slice",  "FB2 Kaleidoscope Slice",  b2_fb2.clone(), -std::f32::consts::PI, std::f32::consts::PI, 0.0, 0.01),

        // =====================================================================
        // Block 2 — FB2 Color
        // =====================================================================
        f!("fb2_hsb_offset_h",     "FB2 Hue Offset",     b2_fb2.clone(), -1.0, 1.0, 0.0, 0.01),
        f!("fb2_hsb_offset_s",     "FB2 Sat Offset",     b2_fb2.clone(), -1.0, 1.0, 0.0, 0.01),
        f!("fb2_hsb_offset_b",     "FB2 Bri Offset",     b2_fb2.clone(), -1.0, 1.0, 0.0, 0.01),
        f!("fb2_hsb_attenuate_h",  "FB2 Hue Attenuate",  b2_fb2.clone(),  0.0, 2.0, 1.0, 0.01),
        f!("fb2_hsb_attenuate_s",  "FB2 Sat Attenuate",  b2_fb2.clone(),  0.0, 2.0, 1.0, 0.01),
        f!("fb2_hsb_attenuate_b",  "FB2 Bri Attenuate",  b2_fb2.clone(),  0.0, 2.0, 1.0, 0.01),
        f!("fb2_hsb_powmap_h",     "FB2 Hue PowMap",     b2_fb2.clone(),  0.0, 2.0, 1.0, 0.01),
        f!("fb2_hsb_powmap_s",     "FB2 Sat PowMap",     b2_fb2.clone(),  0.0, 2.0, 1.0, 0.01),
        f!("fb2_hsb_powmap_b",     "FB2 Bri PowMap",     b2_fb2.clone(),  0.0, 2.0, 1.0, 0.01),
        f!("fb2_hue_shaper",       "FB2 Hue Shaper",     b2_fb2.clone(),  0.0, 2.0, 1.0, 0.01),

        // =====================================================================
        // Block 2 — FB2 Filters
        // =====================================================================
        f!("fb2_blur_amount",       "FB2 Blur Amount",       b2_fb2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("fb2_sharpen_amount",    "FB2 Sharpen Amount",    b2_fb2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("fb2_filters_boost",     "FB2 Filters Boost",     b2_fb2.clone(), 0.0, 1.0, 0.0, 0.01),

        // =====================================================================
        // Block 3 — B1 Re-process
        // =====================================================================
        f!("block1_x_displace",          "B1 X Displace",          b3_b1.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("block1_y_displace",          "B1 Y Displace",          b3_b1.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("block1_z_displace",          "B1 Z Displace",          b3_b1.clone(),  0.0,  4.0, 1.0, 0.01),
        f!("block1_rotate",              "B1 Rotate",              b3_b1.clone(), -std::f32::consts::TAU, std::f32::consts::TAU, 0.0, 0.01),
        f!("block1_shear_xx",            "B1 Shear XX",            b3_b1.clone(), -2.0,  2.0, 1.0, 0.01),
        f!("block1_shear_xy",            "B1 Shear XY",            b3_b1.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("block1_shear_yx",            "B1 Shear YX",            b3_b1.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("block1_shear_yy",            "B1 Shear YY",            b3_b1.clone(), -2.0,  2.0, 1.0, 0.01),
        f!("block1_kaleidoscope_amount", "B1 Kaleidoscope",        b3_b1.clone(),  0.0,  1.0, 0.0, 0.01),
        f!("block1_kaleidoscope_slice",  "B1 Kaleidoscope Slice",  b3_b1.clone(), -std::f32::consts::PI, std::f32::consts::PI, 0.0, 0.01),

        // =====================================================================
        // Block 3 — B1 Colorize
        // =====================================================================
        f!("block1_colorize_band1_h", "B1 Band1 Hue", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block1_colorize_band1_s", "B1 Band1 Sat", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block1_colorize_band1_b", "B1 Band1 Bri", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block1_colorize_band2_h", "B1 Band2 Hue", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block1_colorize_band2_s", "B1 Band2 Sat", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block1_colorize_band2_b", "B1 Band2 Bri", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block1_colorize_band3_h", "B1 Band3 Hue", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block1_colorize_band3_s", "B1 Band3 Sat", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block1_colorize_band3_b", "B1 Band3 Bri", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block1_colorize_band4_h", "B1 Band4 Hue", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block1_colorize_band4_s", "B1 Band4 Sat", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block1_colorize_band4_b", "B1 Band4 Bri", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block1_colorize_band5_h", "B1 Band5 Hue", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block1_colorize_band5_s", "B1 Band5 Sat", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block1_colorize_band5_b", "B1 Band5 Bri", b3_b1.clone(), 0.0, 1.0, 0.0, 0.01),

        // =====================================================================
        // Block 3 — B2 Re-process
        // =====================================================================
        f!("block2_x_displace",          "B2 X Displace",          b3_b2.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("block2_y_displace",          "B2 Y Displace",          b3_b2.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("block2_z_displace",          "B2 Z Displace",          b3_b2.clone(),  0.0,  4.0, 1.0, 0.01),
        f!("block2_rotate",              "B2 Rotate",              b3_b2.clone(), -std::f32::consts::TAU, std::f32::consts::TAU, 0.0, 0.01),
        f!("block2_shear_xx",            "B2 Shear XX",            b3_b2.clone(), -2.0,  2.0, 1.0, 0.01),
        f!("block2_shear_xy",            "B2 Shear XY",            b3_b2.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("block2_shear_yx",            "B2 Shear YX",            b3_b2.clone(), -2.0,  2.0, 0.0, 0.01),
        f!("block2_shear_yy",            "B2 Shear YY",            b3_b2.clone(), -2.0,  2.0, 1.0, 0.01),
        f!("block2_kaleidoscope_amount", "B2 Kaleidoscope",        b3_b2.clone(),  0.0,  1.0, 0.0, 0.01),
        f!("block2_kaleidoscope_slice",  "B2 Kaleidoscope Slice",  b3_b2.clone(), -std::f32::consts::PI, std::f32::consts::PI, 0.0, 0.01),

        // =====================================================================
        // Block 3 — B2 Colorize
        // =====================================================================
        f!("block2_colorize_band1_h", "B2 Band1 Hue", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block2_colorize_band1_s", "B2 Band1 Sat", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block2_colorize_band1_b", "B2 Band1 Bri", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block2_colorize_band2_h", "B2 Band2 Hue", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block2_colorize_band2_s", "B2 Band2 Sat", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block2_colorize_band2_b", "B2 Band2 Bri", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block2_colorize_band3_h", "B2 Band3 Hue", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block2_colorize_band3_s", "B2 Band3 Sat", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block2_colorize_band3_b", "B2 Band3 Bri", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block2_colorize_band4_h", "B2 Band4 Hue", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block2_colorize_band4_s", "B2 Band4 Sat", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block2_colorize_band4_b", "B2 Band4 Bri", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block2_colorize_band5_h", "B2 Band5 Hue", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block2_colorize_band5_s", "B2 Band5 Sat", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),
        f!("block2_colorize_band5_b", "B2 Band5 Bri", b3_b2.clone(), 0.0, 1.0, 0.0, 0.01),

        // =====================================================================
        // Block 3 — Matrix Mixer
        // =====================================================================
        f!("matrix_mix_r_to_r", "Matrix R→R", b3_mat.clone(), -2.0, 2.0, 0.0, 0.01),
        f!("matrix_mix_r_to_g", "Matrix R→G", b3_mat.clone(), -2.0, 2.0, 0.0, 0.01),
        f!("matrix_mix_r_to_b", "Matrix R→B", b3_mat.clone(), -2.0, 2.0, 0.0, 0.01),
        f!("matrix_mix_g_to_r", "Matrix G→R", b3_mat.clone(), -2.0, 2.0, 0.0, 0.01),
        f!("matrix_mix_g_to_g", "Matrix G→G", b3_mat.clone(), -2.0, 2.0, 0.0, 0.01),
        f!("matrix_mix_g_to_b", "Matrix G→B", b3_mat.clone(), -2.0, 2.0, 0.0, 0.01),
        f!("matrix_mix_b_to_r", "Matrix B→R", b3_mat.clone(), -2.0, 2.0, 0.0, 0.01),
        f!("matrix_mix_b_to_g", "Matrix B→G", b3_mat.clone(), -2.0, 2.0, 0.0, 0.01),
        f!("matrix_mix_b_to_b", "Matrix B→B", b3_mat.clone(), -2.0, 2.0, 0.0, 0.01),

        // =====================================================================
        // Block 3 — Final Mix
        // =====================================================================
        f!("final_mix_amount",   "Final Mix Amount",   b3_fin.clone(), 0.0, 1.0, 0.0, 0.01),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_count_in_range() {
        let d = waaaves_parameter_descriptors();
        assert!(d.len() >= 120, "got {} descriptors", d.len());
        assert!(d.len() <= 150, "got {} descriptors", d.len());
    }

    #[test]
    fn descriptor_ids_unique() {
        let d = waaaves_parameter_descriptors();
        let mut ids: Vec<_> = d.iter().map(|p| p.id.clone()).collect();
        ids.sort();
        let unique_len = ids.iter().cloned().collect::<std::collections::HashSet<_>>().len();
        assert_eq!(ids.len(), unique_len, "duplicate parameter IDs found");
    }

    #[test]
    fn all_floats_are_modulatable() {
        let d = waaaves_parameter_descriptors();
        for desc in &d {
            assert!(desc.is_modulatable(), "{} is not modulatable", desc.id);
        }
    }
}
