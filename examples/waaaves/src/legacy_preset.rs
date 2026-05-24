//! Legacy preset import — reads old rustjay-waaaves JSON files.

use crate::params::{Block1Params, Block2Params, Block3Params};
use crate::state::WaaavesState;
use serde::Deserialize;

// ── Helper types with flexible deserialization ─────────────────────────────

/// Deserialises from either `[x, y, z]` or `{"x": x, "y": y, "z": z}`.
#[derive(Debug, Clone, Copy)]
pub struct GlamVec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl<'de> Deserialize<'de> for GlamVec3 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if let Some(arr) = value.as_array() {
            if arr.len() >= 3 {
                return Ok(GlamVec3 {
                    x: arr[0].as_f64().unwrap_or(0.0) as f32,
                    y: arr[1].as_f64().unwrap_or(0.0) as f32,
                    z: arr[2].as_f64().unwrap_or(0.0) as f32,
                });
            }
        }
        #[derive(Deserialize)]
        struct Fields {
            x: f32,
            y: f32,
            z: f32,
        }
        let f = Fields::deserialize(value).map_err(serde::de::Error::custom)?;
        Ok(GlamVec3 { x: f.x, y: f.y, z: f.z })
    }
}

impl From<GlamVec3> for [f32; 3] {
    fn from(v: GlamVec3) -> Self {
        [v.x, v.y, v.z]
    }
}

/// Deserialises from either `[x, y, z, w]` or `{"x": x, "y": y, "z": z, "w": w}`.
#[derive(Debug, Clone, Copy)]
pub struct GlamVec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl<'de> Deserialize<'de> for GlamVec4 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if let Some(arr) = value.as_array() {
            if arr.len() >= 4 {
                return Ok(GlamVec4 {
                    x: arr[0].as_f64().unwrap_or(0.0) as f32,
                    y: arr[1].as_f64().unwrap_or(0.0) as f32,
                    z: arr[2].as_f64().unwrap_or(0.0) as f32,
                    w: arr[3].as_f64().unwrap_or(0.0) as f32,
                });
            }
        }
        #[derive(Deserialize)]
        struct Fields {
            x: f32,
            y: f32,
            z: f32,
            w: f32,
        }
        let f = Fields::deserialize(value).map_err(serde::de::Error::custom)?;
        Ok(GlamVec4 {
            x: f.x,
            y: f.y,
            z: f.z,
            w: f.w,
        })
    }
}

impl From<GlamVec4> for [f32; 4] {
    fn from(v: GlamVec4) -> Self {
        [v.x, v.y, v.z, v.w]
    }
}

// ── Legacy block structs ───────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(default)]
struct LegacyBlock1 {
    ch1_x_displace: f32,
    ch1_y_displace: f32,
    ch1_z_displace: f32,
    ch1_rotate: f32,
    ch1_hsb_attenuate: GlamVec3,
    ch1_posterize: f32,
    ch1_kaleidoscope_amount: f32,
    ch1_kaleidoscope_slice: f32,
    ch1_blur_amount: f32,
    ch1_blur_radius: f32,
    ch1_sharpen_amount: f32,
    ch1_sharpen_radius: f32,
    ch1_filters_boost: f32,
    ch1_h_mirror: bool,
    ch1_v_mirror: bool,
    ch1_h_flip: bool,
    ch1_v_flip: bool,
    ch1_hue_invert: bool,
    ch1_saturation_invert: bool,
    ch1_bright_invert: bool,
    ch1_rgb_invert: bool,
    ch1_geo_overflow: i32,
    ch1_solarize: bool,
    ch1_posterize_switch: bool,
    ch1_hd_aspect_on: bool,
    ch1_input_select: i32,

    ch2_mix_amount: f32,
    ch2_key_value_red: f32,
    ch2_key_value_green: f32,
    ch2_key_value_blue: f32,
    ch2_key_threshold: f32,
    ch2_key_soft: f32,
    ch2_mix_type: i32,
    ch2_mix_overflow: i32,
    ch2_key_order: i32,
    ch2_key_mode: i32,

    ch2_x_displace: f32,
    ch2_y_displace: f32,
    ch2_z_displace: f32,
    ch2_rotate: f32,
    ch2_hsb_attenuate: GlamVec3,
    ch2_posterize: f32,
    ch2_kaleidoscope_amount: f32,
    ch2_kaleidoscope_slice: f32,
    ch2_blur_amount: f32,
    ch2_blur_radius: f32,
    ch2_sharpen_amount: f32,
    ch2_sharpen_radius: f32,
    ch2_filters_boost: f32,
    ch2_h_mirror: bool,
    ch2_v_mirror: bool,
    ch2_h_flip: bool,
    ch2_v_flip: bool,
    ch2_hue_invert: bool,
    ch2_saturation_invert: bool,
    ch2_bright_invert: bool,
    ch2_rgb_invert: bool,
    ch2_geo_overflow: i32,
    ch2_solarize: bool,
    ch2_posterize_switch: bool,
    ch2_hd_aspect_on: bool,
    ch2_input_select: i32,

    fb1_mix_amount: f32,
    fb1_key_value_red: f32,
    fb1_key_value_green: f32,
    fb1_key_value_blue: f32,
    fb1_key_threshold: f32,
    fb1_key_soft: f32,
    fb1_mix_type: i32,
    fb1_mix_overflow: i32,
    fb1_key_order: i32,
    fb1_key_mode: i32,

    fb1_x_displace: f32,
    fb1_y_displace: f32,
    fb1_z_displace: f32,
    fb1_rotate: f32,
    fb1_shear_matrix: GlamVec4,
    fb1_kaleidoscope_amount: f32,
    fb1_kaleidoscope_slice: f32,
    fb1_h_mirror: bool,
    fb1_v_mirror: bool,
    fb1_h_flip: bool,
    fb1_v_flip: bool,
    fb1_rotate_mode: i32,
    fb1_geo_overflow: i32,

    fb1_hsb_offset: GlamVec3,
    fb1_hsb_attenuate: GlamVec3,
    fb1_hsb_powmap: GlamVec3,
    fb1_hue_shaper: f32,
    fb1_posterize: f32,
    fb1_posterize_invert: f32,
    fb1_hue_invert: bool,
    fb1_saturation_invert: bool,
    fb1_bright_invert: bool,
    fb1_posterize_switch: bool,

    fb1_blur_amount: f32,
    fb1_blur_radius: f32,
    fb1_sharpen_amount: f32,
    fb1_sharpen_radius: f32,
    fb1_temporal_filter1_amount: f32,
    fb1_temporal_filter1_resonance: f32,
    fb1_temporal_filter2_amount: f32,
    fb1_temporal_filter2_resonance: f32,
    fb1_filters_boost: f32,

    fb1_delay_time: u32,
    fb1_delay_time_sync: bool,
    fb1_delay_time_division: i32,
}

impl Default for LegacyBlock1 {
    fn default() -> Self {
        Self {
            ch1_x_displace: 0.0,
            ch1_y_displace: 0.0,
            ch1_z_displace: 1.0,
            ch1_rotate: 0.0,
            ch1_hsb_attenuate: GlamVec3 { x: 1.0, y: 1.0, z: 1.0 },
            ch1_posterize: 16.0,
            ch1_kaleidoscope_amount: 0.0,
            ch1_kaleidoscope_slice: 0.0,
            ch1_blur_amount: 0.0,
            ch1_blur_radius: 1.0,
            ch1_sharpen_amount: 0.0,
            ch1_sharpen_radius: 1.0,
            ch1_filters_boost: 0.0,
            ch1_h_mirror: false,
            ch1_v_mirror: false,
            ch1_h_flip: false,
            ch1_v_flip: false,
            ch1_hue_invert: false,
            ch1_saturation_invert: false,
            ch1_bright_invert: false,
            ch1_rgb_invert: false,
            ch1_geo_overflow: 0,
            ch1_solarize: false,
            ch1_posterize_switch: false,
            ch1_hd_aspect_on: false,
            ch1_input_select: 0,

            ch2_mix_amount: 0.0,
            ch2_key_value_red: 0.0,
            ch2_key_value_green: 0.0,
            ch2_key_value_blue: 0.0,
            ch2_key_threshold: 1.0,
            ch2_key_soft: 0.0,
            ch2_mix_type: 0,
            ch2_mix_overflow: 0,
            ch2_key_order: 0,
            ch2_key_mode: 0,

            ch2_x_displace: 0.0,
            ch2_y_displace: 0.0,
            ch2_z_displace: 1.0,
            ch2_rotate: 0.0,
            ch2_hsb_attenuate: GlamVec3 { x: 1.0, y: 1.0, z: 1.0 },
            ch2_posterize: 16.0,
            ch2_kaleidoscope_amount: 0.0,
            ch2_kaleidoscope_slice: 0.0,
            ch2_blur_amount: 0.0,
            ch2_blur_radius: 1.0,
            ch2_sharpen_amount: 0.0,
            ch2_sharpen_radius: 1.0,
            ch2_filters_boost: 0.0,
            ch2_h_mirror: false,
            ch2_v_mirror: false,
            ch2_h_flip: false,
            ch2_v_flip: false,
            ch2_hue_invert: false,
            ch2_saturation_invert: false,
            ch2_bright_invert: false,
            ch2_rgb_invert: false,
            ch2_geo_overflow: 0,
            ch2_solarize: false,
            ch2_posterize_switch: false,
            ch2_hd_aspect_on: false,
            ch2_input_select: 1,

            fb1_mix_amount: 0.0,
            fb1_key_value_red: 0.0,
            fb1_key_value_green: 0.0,
            fb1_key_value_blue: 0.0,
            fb1_key_threshold: 1.0,
            fb1_key_soft: 0.0,
            fb1_mix_type: 0,
            fb1_mix_overflow: 0,
            fb1_key_order: 0,
            fb1_key_mode: 0,

            fb1_x_displace: 0.0,
            fb1_y_displace: 0.0,
            fb1_z_displace: 1.0,
            fb1_rotate: 0.0,
            fb1_shear_matrix: GlamVec4 { x: 1.0, y: 0.0, z: 0.0, w: 1.0 },
            fb1_kaleidoscope_amount: 0.0,
            fb1_kaleidoscope_slice: 0.0,
            fb1_h_mirror: false,
            fb1_v_mirror: false,
            fb1_h_flip: false,
            fb1_v_flip: false,
            fb1_rotate_mode: 0,
            fb1_geo_overflow: 0,

            fb1_hsb_offset: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },
            fb1_hsb_attenuate: GlamVec3 { x: 1.0, y: 1.0, z: 1.0 },
            fb1_hsb_powmap: GlamVec3 { x: 1.0, y: 1.0, z: 1.0 },
            fb1_hue_shaper: 1.0,
            fb1_posterize: 16.0,
            fb1_posterize_invert: 1.0 / 16.0,
            fb1_hue_invert: false,
            fb1_saturation_invert: false,
            fb1_bright_invert: false,
            fb1_posterize_switch: false,

            fb1_blur_amount: 0.0,
            fb1_blur_radius: 1.0,
            fb1_sharpen_amount: 0.0,
            fb1_sharpen_radius: 1.0,
            fb1_temporal_filter1_amount: 0.0,
            fb1_temporal_filter1_resonance: 0.0,
            fb1_temporal_filter2_amount: 0.0,
            fb1_temporal_filter2_resonance: 0.0,
            fb1_filters_boost: 0.0,

            fb1_delay_time: 1,
            fb1_delay_time_sync: false,
            fb1_delay_time_division: 2,
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct LegacyBlock2 {
    block2_input_x_displace: f32,
    block2_input_y_displace: f32,
    block2_input_z_displace: f32,
    block2_input_rotate: f32,
    block2_input_hsb_attenuate: GlamVec3,
    block2_input_posterize: f32,
    block2_input_kaleidoscope_amount: f32,
    block2_input_kaleidoscope_slice: f32,
    block2_input_blur_amount: f32,
    block2_input_blur_radius: f32,
    block2_input_sharpen_amount: f32,
    block2_input_sharpen_radius: f32,
    block2_input_filters_boost: f32,
    block2_input_h_mirror: bool,
    block2_input_v_mirror: bool,
    block2_input_h_flip: bool,
    block2_input_v_flip: bool,
    block2_input_hue_invert: bool,
    block2_input_saturation_invert: bool,
    block2_input_bright_invert: bool,
    block2_input_rgb_invert: bool,
    block2_input_geo_overflow: i32,
    block2_input_solarize: bool,
    block2_input_posterize_switch: bool,
    block2_input_hd_aspect_on: bool,

    fb2_mix_amount: f32,
    fb2_key_value: Option<GlamVec3>,
    fb2_key_value_red: f32,
    fb2_key_value_green: f32,
    fb2_key_value_blue: f32,
    fb2_key_threshold: f32,
    fb2_key_soft: f32,
    fb2_mix_type: i32,
    fb2_mix_overflow: i32,
    fb2_key_mode: i32,
    fb2_key_order: i32,

    fb2_x_displace: f32,
    fb2_y_displace: f32,
    fb2_z_displace: f32,
    fb2_rotate: f32,
    fb2_shear_matrix: GlamVec4,
    fb2_kaleidoscope_amount: f32,
    fb2_kaleidoscope_slice: f32,
    fb2_h_mirror: bool,
    fb2_v_mirror: bool,
    fb2_h_flip: bool,
    fb2_v_flip: bool,
    fb2_rotate_mode: i32,
    fb2_geo_overflow: i32,

    fb2_hsb_offset: GlamVec3,
    fb2_hsb_attenuate: GlamVec3,
    fb2_hsb_powmap: GlamVec3,
    fb2_hue_shaper: f32,
    fb2_posterize: f32,
    fb2_posterize_invert: f32,
    fb2_posterize_switch: bool,
    fb2_hue_invert: bool,
    fb2_saturation_invert: bool,
    fb2_bright_invert: bool,
    fb2_rgb_invert: bool,

    fb2_blur_amount: f32,
    fb2_blur_radius: f32,
    fb2_sharpen_amount: f32,
    fb2_sharpen_radius: f32,
    fb2_temporal_filter1_amount: f32,
    fb2_temporal_filter1_resonance: f32,
    fb2_temporal_filter2_amount: f32,
    fb2_temporal_filter2_resonance: f32,
    fb2_filters_boost: f32,

    fb2_delay_time: u32,
    fb2_delay_time_sync: bool,
    fb2_delay_time_division: i32,

    block2_input_select: i32,
}

impl Default for LegacyBlock2 {
    fn default() -> Self {
        Self {
            block2_input_x_displace: 0.0,
            block2_input_y_displace: 0.0,
            block2_input_z_displace: 1.0,
            block2_input_rotate: 0.0,
            block2_input_hsb_attenuate: GlamVec3 { x: 1.0, y: 1.0, z: 1.0 },
            block2_input_posterize: 16.0,
            block2_input_kaleidoscope_amount: 0.0,
            block2_input_kaleidoscope_slice: 0.0,
            block2_input_blur_amount: 0.0,
            block2_input_blur_radius: 1.0,
            block2_input_sharpen_amount: 0.0,
            block2_input_sharpen_radius: 1.0,
            block2_input_filters_boost: 0.0,
            block2_input_h_mirror: false,
            block2_input_v_mirror: false,
            block2_input_h_flip: false,
            block2_input_v_flip: false,
            block2_input_hue_invert: false,
            block2_input_saturation_invert: false,
            block2_input_bright_invert: false,
            block2_input_rgb_invert: false,
            block2_input_geo_overflow: 0,
            block2_input_solarize: false,
            block2_input_posterize_switch: false,
            block2_input_hd_aspect_on: false,

            fb2_mix_amount: 0.0,
            fb2_key_value: None,
            fb2_key_value_red: 0.0,
            fb2_key_value_green: 0.0,
            fb2_key_value_blue: 0.0,
            fb2_key_threshold: 1.0,
            fb2_key_soft: 0.0,
            fb2_mix_type: 0,
            fb2_mix_overflow: 0,
            fb2_key_mode: 0,
            fb2_key_order: 0,

            fb2_x_displace: 0.0,
            fb2_y_displace: 0.0,
            fb2_z_displace: 1.0,
            fb2_rotate: 0.0,
            fb2_shear_matrix: GlamVec4 { x: 1.0, y: 0.0, z: 0.0, w: 1.0 },
            fb2_kaleidoscope_amount: 0.0,
            fb2_kaleidoscope_slice: 0.0,
            fb2_h_mirror: false,
            fb2_v_mirror: false,
            fb2_h_flip: false,
            fb2_v_flip: false,
            fb2_rotate_mode: 0,
            fb2_geo_overflow: 0,

            fb2_hsb_offset: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },
            fb2_hsb_attenuate: GlamVec3 { x: 1.0, y: 1.0, z: 1.0 },
            fb2_hsb_powmap: GlamVec3 { x: 1.0, y: 1.0, z: 1.0 },
            fb2_hue_shaper: 1.0,
            fb2_posterize: 16.0,
            fb2_posterize_invert: 1.0 / 16.0,
            fb2_posterize_switch: false,
            fb2_hue_invert: false,
            fb2_saturation_invert: false,
            fb2_bright_invert: false,
            fb2_rgb_invert: false,

            fb2_blur_amount: 0.0,
            fb2_blur_radius: 1.0,
            fb2_sharpen_amount: 0.0,
            fb2_sharpen_radius: 1.0,
            fb2_temporal_filter1_amount: 0.0,
            fb2_temporal_filter1_resonance: 0.0,
            fb2_temporal_filter2_amount: 0.0,
            fb2_temporal_filter2_resonance: 0.0,
            fb2_filters_boost: 0.0,

            fb2_delay_time: 1,
            fb2_delay_time_sync: false,
            fb2_delay_time_division: 2,

            block2_input_select: 0,
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct LegacyBlock3 {
    block1_x_displace: f32,
    block1_y_displace: f32,
    block1_z_displace: f32,
    block1_rotate: f32,
    block1_shear_matrix: GlamVec4,
    block1_kaleidoscope_amount: f32,
    block1_kaleidoscope_slice: f32,
    block1_h_mirror: bool,
    block1_v_mirror: bool,
    block1_rotate_mode: i32,
    block1_geo_overflow: i32,
    block1_h_flip: bool,
    block1_v_flip: bool,

    block1_colorize_switch: bool,
    block1_colorize_hsb_rgb: i32,
    block1_colorize_band1: GlamVec3,
    block1_colorize_band2: GlamVec3,
    block1_colorize_band3: GlamVec3,
    block1_colorize_band4: GlamVec3,
    block1_colorize_band5: GlamVec3,

    block1_blur_amount: f32,
    block1_blur_radius: f32,
    block1_sharpen_amount: f32,
    block1_sharpen_radius: f32,
    block1_filters_boost: f32,
    block1_dither: f32,
    block1_dither_switch: bool,
    block1_dither_type: i32,

    block2_x_displace: f32,
    block2_y_displace: f32,
    block2_z_displace: f32,
    block2_rotate: f32,
    block2_shear_matrix: GlamVec4,
    block2_kaleidoscope_amount: f32,
    block2_kaleidoscope_slice: f32,
    block2_h_mirror: bool,
    block2_v_mirror: bool,
    block2_rotate_mode: i32,
    block2_geo_overflow: i32,
    block2_h_flip: bool,
    block2_v_flip: bool,

    block2_colorize_switch: bool,
    block2_colorize_hsb_rgb: i32,
    block2_colorize_band1: GlamVec3,
    block2_colorize_band2: GlamVec3,
    block2_colorize_band3: GlamVec3,
    block2_colorize_band4: GlamVec3,
    block2_colorize_band5: GlamVec3,

    block2_blur_amount: f32,
    block2_blur_radius: f32,
    block2_sharpen_amount: f32,
    block2_sharpen_radius: f32,
    block2_filters_boost: f32,
    block2_dither: f32,
    block2_dither_switch: bool,
    block2_dither_type: i32,

    matrix_mix_type: i32,
    matrix_mix_overflow: i32,
    bg_rgb_into_fg_red: GlamVec3,
    bg_rgb_into_fg_green: GlamVec3,
    bg_rgb_into_fg_blue: GlamVec3,

    final_mix_amount: f32,
    final_key_value: GlamVec3,
    final_key_threshold: f32,
    final_key_soft: f32,
    final_mix_type: i32,
    final_mix_overflow: i32,
    final_key_order: i32,
    final_key_mode: i32,

    final_dither: f32,
    final_dither_switch: bool,
    final_dither_type: i32,
}

impl Default for LegacyBlock3 {
    fn default() -> Self {
        Self {
            block1_x_displace: 0.0,
            block1_y_displace: 0.0,
            block1_z_displace: 1.0,
            block1_rotate: 0.0,
            block1_shear_matrix: GlamVec4 { x: 1.0, y: 0.0, z: 0.0, w: 1.0 },
            block1_kaleidoscope_amount: 0.0,
            block1_kaleidoscope_slice: 0.0,
            block1_h_mirror: false,
            block1_v_mirror: false,
            block1_rotate_mode: 0,
            block1_geo_overflow: 0,
            block1_h_flip: false,
            block1_v_flip: false,

            block1_colorize_switch: false,
            block1_colorize_hsb_rgb: 0,
            block1_colorize_band1: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },
            block1_colorize_band2: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },
            block1_colorize_band3: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },
            block1_colorize_band4: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },
            block1_colorize_band5: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },

            block1_blur_amount: 0.0,
            block1_blur_radius: 1.0,
            block1_sharpen_amount: 0.0,
            block1_sharpen_radius: 1.0,
            block1_filters_boost: 0.0,
            block1_dither: 16.0,
            block1_dither_switch: false,
            block1_dither_type: 0,

            block2_x_displace: 0.0,
            block2_y_displace: 0.0,
            block2_z_displace: 1.0,
            block2_rotate: 0.0,
            block2_shear_matrix: GlamVec4 { x: 1.0, y: 0.0, z: 0.0, w: 1.0 },
            block2_kaleidoscope_amount: 0.0,
            block2_kaleidoscope_slice: 0.0,
            block2_h_mirror: false,
            block2_v_mirror: false,
            block2_rotate_mode: 0,
            block2_geo_overflow: 0,
            block2_h_flip: false,
            block2_v_flip: false,

            block2_colorize_switch: false,
            block2_colorize_hsb_rgb: 0,
            block2_colorize_band1: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },
            block2_colorize_band2: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },
            block2_colorize_band3: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },
            block2_colorize_band4: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },
            block2_colorize_band5: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },

            block2_blur_amount: 0.0,
            block2_blur_radius: 1.0,
            block2_sharpen_amount: 0.0,
            block2_sharpen_radius: 1.0,
            block2_filters_boost: 0.0,
            block2_dither: 16.0,
            block2_dither_switch: false,
            block2_dither_type: 0,

            matrix_mix_type: 0,
            matrix_mix_overflow: 0,
            bg_rgb_into_fg_red: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },
            bg_rgb_into_fg_green: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },
            bg_rgb_into_fg_blue: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },

            final_mix_amount: 0.0,
            final_key_value: GlamVec3 { x: 0.0, y: 0.0, z: 0.0 },
            final_key_threshold: 1.0,
            final_key_soft: 0.0,
            final_mix_type: 0,
            final_mix_overflow: 0,
            final_key_order: 0,
            final_key_mode: 0,

            final_dither: 16.0,
            final_dither_switch: false,
            final_dither_type: 0,
        }
    }
}

#[derive(Deserialize)]
struct LegacyPreset {
    block1: LegacyBlock1,
    block2: LegacyBlock2,
    block3: LegacyBlock3,
}

// ── From impls ─────────────────────────────────────────────────────────────

impl From<LegacyBlock1> for Block1Params {
    fn from(l: LegacyBlock1) -> Self {
        Self {
            ch1_x_displace: l.ch1_x_displace,
            ch1_y_displace: l.ch1_y_displace,
            ch1_z_displace: l.ch1_z_displace,
            ch1_rotate: l.ch1_rotate,
            ch1_hsb_attenuate_h: l.ch1_hsb_attenuate.x,
            ch1_hsb_attenuate_s: l.ch1_hsb_attenuate.y,
            ch1_hsb_attenuate_b: l.ch1_hsb_attenuate.z,
            ch1_posterize: l.ch1_posterize,
            ch1_kaleidoscope_amount: l.ch1_kaleidoscope_amount,
            ch1_kaleidoscope_slice: l.ch1_kaleidoscope_slice,
            ch1_blur_amount: l.ch1_blur_amount,
            ch1_blur_radius: l.ch1_blur_radius,
            ch1_sharpen_amount: l.ch1_sharpen_amount,
            ch1_sharpen_radius: l.ch1_sharpen_radius,
            ch1_filters_boost: l.ch1_filters_boost,
            ch1_h_mirror: l.ch1_h_mirror,
            ch1_v_mirror: l.ch1_v_mirror,
            ch1_h_flip: l.ch1_h_flip,
            ch1_v_flip: l.ch1_v_flip,
            ch1_hue_invert: l.ch1_hue_invert,
            ch1_saturation_invert: l.ch1_saturation_invert,
            ch1_bright_invert: l.ch1_bright_invert,
            ch1_rgb_invert: l.ch1_rgb_invert,
            ch1_geo_overflow: l.ch1_geo_overflow,
            ch1_solarize: l.ch1_solarize,
            ch1_posterize_switch: l.ch1_posterize_switch,
            ch1_hd_aspect_on: l.ch1_hd_aspect_on,
            ch1_input_select: l.ch1_input_select,

            ch2_mix_amount: l.ch2_mix_amount,
            ch2_key_value_r: l.ch2_key_value_red,
            ch2_key_value_g: l.ch2_key_value_green,
            ch2_key_value_b: l.ch2_key_value_blue,
            ch2_key_threshold: l.ch2_key_threshold,
            ch2_key_soft: l.ch2_key_soft,
            ch2_mix_type: l.ch2_mix_type,
            ch2_mix_overflow: l.ch2_mix_overflow,
            ch2_key_order: l.ch2_key_order,
            ch2_key_mode: l.ch2_key_mode,

            ch2_x_displace: l.ch2_x_displace,
            ch2_y_displace: l.ch2_y_displace,
            ch2_z_displace: l.ch2_z_displace,
            ch2_rotate: l.ch2_rotate,
            ch2_hsb_attenuate_h: l.ch2_hsb_attenuate.x,
            ch2_hsb_attenuate_s: l.ch2_hsb_attenuate.y,
            ch2_hsb_attenuate_b: l.ch2_hsb_attenuate.z,
            ch2_posterize: l.ch2_posterize,
            ch2_kaleidoscope_amount: l.ch2_kaleidoscope_amount,
            ch2_kaleidoscope_slice: l.ch2_kaleidoscope_slice,
            ch2_blur_amount: l.ch2_blur_amount,
            ch2_blur_radius: l.ch2_blur_radius,
            ch2_sharpen_amount: l.ch2_sharpen_amount,
            ch2_sharpen_radius: l.ch2_sharpen_radius,
            ch2_filters_boost: l.ch2_filters_boost,
            ch2_h_mirror: l.ch2_h_mirror,
            ch2_v_mirror: l.ch2_v_mirror,
            ch2_h_flip: l.ch2_h_flip,
            ch2_v_flip: l.ch2_v_flip,
            ch2_hue_invert: l.ch2_hue_invert,
            ch2_saturation_invert: l.ch2_saturation_invert,
            ch2_bright_invert: l.ch2_bright_invert,
            ch2_rgb_invert: l.ch2_rgb_invert,
            ch2_geo_overflow: l.ch2_geo_overflow,
            ch2_solarize: l.ch2_solarize,
            ch2_posterize_switch: l.ch2_posterize_switch,
            ch2_hd_aspect_on: l.ch2_hd_aspect_on,
            ch2_input_select: l.ch2_input_select,

            fb1_mix_amount: l.fb1_mix_amount,
            fb1_key_value_r: l.fb1_key_value_red,
            fb1_key_value_g: l.fb1_key_value_green,
            fb1_key_value_b: l.fb1_key_value_blue,
            fb1_key_threshold: l.fb1_key_threshold,
            fb1_key_soft: l.fb1_key_soft,
            fb1_mix_type: l.fb1_mix_type,
            fb1_mix_overflow: l.fb1_mix_overflow,
            fb1_key_order: l.fb1_key_order,
            fb1_key_mode: l.fb1_key_mode,

            fb1_x_displace: l.fb1_x_displace,
            fb1_y_displace: l.fb1_y_displace,
            fb1_z_displace: l.fb1_z_displace,
            fb1_rotate: l.fb1_rotate,
            fb1_shear_xx: l.fb1_shear_matrix.x,
            fb1_shear_xy: l.fb1_shear_matrix.y,
            fb1_shear_yx: l.fb1_shear_matrix.z,
            fb1_shear_yy: l.fb1_shear_matrix.w,
            fb1_kaleidoscope_amount: l.fb1_kaleidoscope_amount,
            fb1_kaleidoscope_slice: l.fb1_kaleidoscope_slice,
            fb1_h_mirror: l.fb1_h_mirror,
            fb1_v_mirror: l.fb1_v_mirror,
            fb1_h_flip: l.fb1_h_flip,
            fb1_v_flip: l.fb1_v_flip,
            fb1_rotate_mode: l.fb1_rotate_mode,
            fb1_geo_overflow: l.fb1_geo_overflow,

            fb1_hsb_offset_h: l.fb1_hsb_offset.x,
            fb1_hsb_offset_s: l.fb1_hsb_offset.y,
            fb1_hsb_offset_b: l.fb1_hsb_offset.z,
            fb1_hsb_attenuate_h: l.fb1_hsb_attenuate.x,
            fb1_hsb_attenuate_s: l.fb1_hsb_attenuate.y,
            fb1_hsb_attenuate_b: l.fb1_hsb_attenuate.z,
            fb1_hsb_powmap_h: l.fb1_hsb_powmap.x,
            fb1_hsb_powmap_s: l.fb1_hsb_powmap.y,
            fb1_hsb_powmap_b: l.fb1_hsb_powmap.z,
            fb1_hue_shaper: l.fb1_hue_shaper,
            fb1_posterize: l.fb1_posterize,
            fb1_posterize_invert: l.fb1_posterize_invert,
            fb1_hue_invert: l.fb1_hue_invert,
            fb1_saturation_invert: l.fb1_saturation_invert,
            fb1_bright_invert: l.fb1_bright_invert,
            fb1_posterize_switch: l.fb1_posterize_switch,

            fb1_blur_amount: l.fb1_blur_amount,
            fb1_blur_radius: l.fb1_blur_radius,
            fb1_sharpen_amount: l.fb1_sharpen_amount,
            fb1_sharpen_radius: l.fb1_sharpen_radius,
            fb1_temporal1_amount: l.fb1_temporal_filter1_amount,
            fb1_temporal1_resonance: l.fb1_temporal_filter1_resonance,
            fb1_temporal2_amount: l.fb1_temporal_filter2_amount,
            fb1_temporal2_resonance: l.fb1_temporal_filter2_resonance,
            fb1_filters_boost: l.fb1_filters_boost,

            fb1_delay_time: l.fb1_delay_time,
            fb1_delay_time_sync: l.fb1_delay_time_sync,
            fb1_delay_time_division: l.fb1_delay_time_division,
        }
    }
}

impl From<LegacyBlock2> for Block2Params {
    fn from(l: LegacyBlock2) -> Self {
        let (fb2_kr, fb2_kg, fb2_kb) = if let Some(v) = l.fb2_key_value {
            (v.x, v.y, v.z)
        } else {
            (l.fb2_key_value_red, l.fb2_key_value_green, l.fb2_key_value_blue)
        };
        Self {
            block2_input_x_displace: l.block2_input_x_displace,
            block2_input_y_displace: l.block2_input_y_displace,
            block2_input_z_displace: l.block2_input_z_displace,
            block2_input_rotate: l.block2_input_rotate,
            block2_input_hsb_attenuate_h: l.block2_input_hsb_attenuate.x,
            block2_input_hsb_attenuate_s: l.block2_input_hsb_attenuate.y,
            block2_input_hsb_attenuate_b: l.block2_input_hsb_attenuate.z,
            block2_input_posterize: l.block2_input_posterize,
            block2_input_kaleidoscope_amount: l.block2_input_kaleidoscope_amount,
            block2_input_kaleidoscope_slice: l.block2_input_kaleidoscope_slice,
            block2_input_blur_amount: l.block2_input_blur_amount,
            block2_input_blur_radius: l.block2_input_blur_radius,
            block2_input_sharpen_amount: l.block2_input_sharpen_amount,
            block2_input_sharpen_radius: l.block2_input_sharpen_radius,
            block2_input_filters_boost: l.block2_input_filters_boost,
            block2_input_h_mirror: l.block2_input_h_mirror,
            block2_input_v_mirror: l.block2_input_v_mirror,
            block2_input_h_flip: l.block2_input_h_flip,
            block2_input_v_flip: l.block2_input_v_flip,
            block2_input_hue_invert: l.block2_input_hue_invert,
            block2_input_saturation_invert: l.block2_input_saturation_invert,
            block2_input_bright_invert: l.block2_input_bright_invert,
            block2_input_rgb_invert: l.block2_input_rgb_invert,
            block2_input_geo_overflow: l.block2_input_geo_overflow,
            block2_input_solarize: l.block2_input_solarize,
            block2_input_posterize_switch: l.block2_input_posterize_switch,
            block2_input_hd_aspect_on: l.block2_input_hd_aspect_on,

            fb2_mix_amount: l.fb2_mix_amount,
            fb2_key_value_r: fb2_kr,
            fb2_key_value_g: fb2_kg,
            fb2_key_value_b: fb2_kb,
            fb2_key_threshold: l.fb2_key_threshold,
            fb2_key_soft: l.fb2_key_soft,
            fb2_mix_type: l.fb2_mix_type,
            fb2_mix_overflow: l.fb2_mix_overflow,
            fb2_key_mode: l.fb2_key_mode,
            fb2_key_order: l.fb2_key_order,

            fb2_x_displace: l.fb2_x_displace,
            fb2_y_displace: l.fb2_y_displace,
            fb2_z_displace: l.fb2_z_displace,
            fb2_rotate: l.fb2_rotate,
            fb2_shear_xx: l.fb2_shear_matrix.x,
            fb2_shear_xy: l.fb2_shear_matrix.y,
            fb2_shear_yx: l.fb2_shear_matrix.z,
            fb2_shear_yy: l.fb2_shear_matrix.w,
            fb2_kaleidoscope_amount: l.fb2_kaleidoscope_amount,
            fb2_kaleidoscope_slice: l.fb2_kaleidoscope_slice,
            fb2_h_mirror: l.fb2_h_mirror,
            fb2_v_mirror: l.fb2_v_mirror,
            fb2_h_flip: l.fb2_h_flip,
            fb2_v_flip: l.fb2_v_flip,
            fb2_rotate_mode: l.fb2_rotate_mode,
            fb2_geo_overflow: l.fb2_geo_overflow,

            fb2_hsb_offset_h: l.fb2_hsb_offset.x,
            fb2_hsb_offset_s: l.fb2_hsb_offset.y,
            fb2_hsb_offset_b: l.fb2_hsb_offset.z,
            fb2_hsb_attenuate_h: l.fb2_hsb_attenuate.x,
            fb2_hsb_attenuate_s: l.fb2_hsb_attenuate.y,
            fb2_hsb_attenuate_b: l.fb2_hsb_attenuate.z,
            fb2_hsb_powmap_h: l.fb2_hsb_powmap.x,
            fb2_hsb_powmap_s: l.fb2_hsb_powmap.y,
            fb2_hsb_powmap_b: l.fb2_hsb_powmap.z,
            fb2_hue_shaper: l.fb2_hue_shaper,
            fb2_posterize: l.fb2_posterize,
            fb2_posterize_invert: l.fb2_posterize_invert,
            fb2_posterize_switch: l.fb2_posterize_switch,
            fb2_hue_invert: l.fb2_hue_invert,
            fb2_saturation_invert: l.fb2_saturation_invert,
            fb2_bright_invert: l.fb2_bright_invert,
            fb2_rgb_invert: l.fb2_rgb_invert,

            fb2_blur_amount: l.fb2_blur_amount,
            fb2_blur_radius: l.fb2_blur_radius,
            fb2_sharpen_amount: l.fb2_sharpen_amount,
            fb2_sharpen_radius: l.fb2_sharpen_radius,
            fb2_temporal1_amount: l.fb2_temporal_filter1_amount,
            fb2_temporal1_resonance: l.fb2_temporal_filter1_resonance,
            fb2_temporal2_amount: l.fb2_temporal_filter2_amount,
            fb2_temporal2_resonance: l.fb2_temporal_filter2_resonance,
            fb2_filters_boost: l.fb2_filters_boost,

            fb2_delay_time: l.fb2_delay_time,
            fb2_delay_time_sync: l.fb2_delay_time_sync,
            fb2_delay_time_division: l.fb2_delay_time_division,

            block2_input_select: l.block2_input_select,
        }
    }
}

impl From<LegacyBlock3> for Block3Params {
    fn from(l: LegacyBlock3) -> Self {
        Self {
            block1_x_displace: l.block1_x_displace,
            block1_y_displace: l.block1_y_displace,
            block1_z_displace: l.block1_z_displace,
            block1_rotate: l.block1_rotate,
            block1_shear_xx: l.block1_shear_matrix.x,
            block1_shear_xy: l.block1_shear_matrix.y,
            block1_shear_yx: l.block1_shear_matrix.z,
            block1_shear_yy: l.block1_shear_matrix.w,
            block1_kaleidoscope_amount: l.block1_kaleidoscope_amount,
            block1_kaleidoscope_slice: l.block1_kaleidoscope_slice,
            block1_h_mirror: l.block1_h_mirror,
            block1_v_mirror: l.block1_v_mirror,
            block1_rotate_mode: l.block1_rotate_mode,
            block1_geo_overflow: l.block1_geo_overflow,
            block1_h_flip: l.block1_h_flip,
            block1_v_flip: l.block1_v_flip,

            block1_colorize_switch: l.block1_colorize_switch,
            block1_colorize_hsb_rgb: l.block1_colorize_hsb_rgb,
            block1_colorize_band1_h: l.block1_colorize_band1.x,
            block1_colorize_band1_s: l.block1_colorize_band1.y,
            block1_colorize_band1_b: l.block1_colorize_band1.z,
            block1_colorize_band2_h: l.block1_colorize_band2.x,
            block1_colorize_band2_s: l.block1_colorize_band2.y,
            block1_colorize_band2_b: l.block1_colorize_band2.z,
            block1_colorize_band3_h: l.block1_colorize_band3.x,
            block1_colorize_band3_s: l.block1_colorize_band3.y,
            block1_colorize_band3_b: l.block1_colorize_band3.z,
            block1_colorize_band4_h: l.block1_colorize_band4.x,
            block1_colorize_band4_s: l.block1_colorize_band4.y,
            block1_colorize_band4_b: l.block1_colorize_band4.z,
            block1_colorize_band5_h: l.block1_colorize_band5.x,
            block1_colorize_band5_s: l.block1_colorize_band5.y,
            block1_colorize_band5_b: l.block1_colorize_band5.z,

            block1_blur_amount: l.block1_blur_amount,
            block1_blur_radius: l.block1_blur_radius,
            block1_sharpen_amount: l.block1_sharpen_amount,
            block1_sharpen_radius: l.block1_sharpen_radius,
            block1_filters_boost: l.block1_filters_boost,
            block1_dither: l.block1_dither,
            block1_dither_switch: l.block1_dither_switch,
            block1_dither_type: l.block1_dither_type,

            block2_x_displace: l.block2_x_displace,
            block2_y_displace: l.block2_y_displace,
            block2_z_displace: l.block2_z_displace,
            block2_rotate: l.block2_rotate,
            block2_shear_xx: l.block2_shear_matrix.x,
            block2_shear_xy: l.block2_shear_matrix.y,
            block2_shear_yx: l.block2_shear_matrix.z,
            block2_shear_yy: l.block2_shear_matrix.w,
            block2_kaleidoscope_amount: l.block2_kaleidoscope_amount,
            block2_kaleidoscope_slice: l.block2_kaleidoscope_slice,
            block2_h_mirror: l.block2_h_mirror,
            block2_v_mirror: l.block2_v_mirror,
            block2_rotate_mode: l.block2_rotate_mode,
            block2_geo_overflow: l.block2_geo_overflow,
            block2_h_flip: l.block2_h_flip,
            block2_v_flip: l.block2_v_flip,

            block2_colorize_switch: l.block2_colorize_switch,
            block2_colorize_hsb_rgb: l.block2_colorize_hsb_rgb,
            block2_colorize_band1_h: l.block2_colorize_band1.x,
            block2_colorize_band1_s: l.block2_colorize_band1.y,
            block2_colorize_band1_b: l.block2_colorize_band1.z,
            block2_colorize_band2_h: l.block2_colorize_band2.x,
            block2_colorize_band2_s: l.block2_colorize_band2.y,
            block2_colorize_band2_b: l.block2_colorize_band2.z,
            block2_colorize_band3_h: l.block2_colorize_band3.x,
            block2_colorize_band3_s: l.block2_colorize_band3.y,
            block2_colorize_band3_b: l.block2_colorize_band3.z,
            block2_colorize_band4_h: l.block2_colorize_band4.x,
            block2_colorize_band4_s: l.block2_colorize_band4.y,
            block2_colorize_band4_b: l.block2_colorize_band4.z,
            block2_colorize_band5_h: l.block2_colorize_band5.x,
            block2_colorize_band5_s: l.block2_colorize_band5.y,
            block2_colorize_band5_b: l.block2_colorize_band5.z,

            block2_blur_amount: l.block2_blur_amount,
            block2_blur_radius: l.block2_blur_radius,
            block2_sharpen_amount: l.block2_sharpen_amount,
            block2_sharpen_radius: l.block2_sharpen_radius,
            block2_filters_boost: l.block2_filters_boost,
            block2_dither: l.block2_dither,
            block2_dither_switch: l.block2_dither_switch,
            block2_dither_type: l.block2_dither_type,

            matrix_mix_type: l.matrix_mix_type,
            matrix_mix_overflow: l.matrix_mix_overflow,
            matrix_mix_r_to_r: l.bg_rgb_into_fg_red.x,
            matrix_mix_r_to_g: l.bg_rgb_into_fg_green.x,
            matrix_mix_r_to_b: l.bg_rgb_into_fg_blue.x,
            matrix_mix_g_to_r: l.bg_rgb_into_fg_red.y,
            matrix_mix_g_to_g: l.bg_rgb_into_fg_green.y,
            matrix_mix_g_to_b: l.bg_rgb_into_fg_blue.y,
            matrix_mix_b_to_r: l.bg_rgb_into_fg_red.z,
            matrix_mix_b_to_g: l.bg_rgb_into_fg_green.z,
            matrix_mix_b_to_b: l.bg_rgb_into_fg_blue.z,

            final_mix_amount: l.final_mix_amount,
            final_key_value_r: l.final_key_value.x,
            final_key_value_g: l.final_key_value.y,
            final_key_value_b: l.final_key_value.z,
            final_key_threshold: l.final_key_threshold,
            final_key_soft: l.final_key_soft,
            final_mix_type: l.final_mix_type,
            final_mix_overflow: l.final_mix_overflow,
            final_key_order: l.final_key_order,
            final_key_mode: l.final_key_mode,

            final_dither: l.final_dither,
            final_dither_switch: l.final_dither_switch,
            final_dither_type: l.final_dither_type,
        }
    }
}

// ── Public import function ─────────────────────────────────────────────────

pub fn import_legacy_preset(json: &str) -> anyhow::Result<WaaavesState> {
    let legacy: LegacyPreset = serde_json::from_str(json)
        .map_err(|e| anyhow::anyhow!("Failed to parse legacy waaaves preset: {e}"))?;
    Ok(WaaavesState {
        block1: legacy.block1.into(),
        block2: legacy.block2.into(),
        block3: legacy.block3.into(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_jfish() {
        let json = include_str!("../../../../rustjay-waaaves/presets/Default/jfish.json");
        let state = import_legacy_preset(json).unwrap();
        // Spot-check 10 fields
        assert!((state.block1.ch1_hsb_attenuate_h - 1.72).abs() < 0.01);
        assert!((state.block1.ch1_hsb_attenuate_s - 0.16).abs() < 0.01);
        assert!((state.block1.ch1_hsb_attenuate_b - 1.32).abs() < 0.01);
        assert!((state.block1.ch1_rotate - 0.0).abs() < 0.001);
        assert!((state.block1.fb1_mix_amount - 0.102).abs() < 0.001);
        assert_eq!(state.block1.fb1_delay_time, 18);
        assert!(state.block1.fb1_delay_time_sync);
        assert!((state.block1.ch2_mix_amount - 0.434).abs() < 0.001);
        assert!((state.block2.fb2_mix_amount - 0.052).abs() < 0.001);
        assert_eq!(state.block2.fb2_delay_time, 9);
        assert!((state.block3.final_mix_amount - 0.0).abs() < 0.001);
    }

    #[test]
    fn import_total_recall() {
        let json = include_str!("../../../../rustjay-waaaves/presets/Default/totalRecall.json");
        let state = import_legacy_preset(json).unwrap();
        // Spot-check 10 fields
        assert!((state.block3.matrix_mix_r_to_r - 0.116).abs() < 0.01);
        assert!((state.block3.matrix_mix_g_to_r - 0.278).abs() < 0.01);
        assert!((state.block3.matrix_mix_b_to_r - 0.284).abs() < 0.01);
        assert!((state.block1.ch1_rotate - 0.0).abs() < 0.001);
        assert!((state.block1.fb1_mix_amount - 0.268).abs() < 0.001);
        assert_eq!(state.block1.fb1_delay_time, 1);
        assert!((state.block1.ch2_mix_amount - 0.128).abs() < 0.001);
        assert!((state.block2.fb2_mix_amount - 0.0).abs() < 0.001);
        assert_eq!(state.block2.fb2_delay_time, 1);
        assert_eq!(state.block3.matrix_mix_type, 2);
    }

    #[test]
    fn import_scanline() {
        let json = include_str!("../../../../rustjay-waaaves/presets/Default/scanline.json");
        let state = import_legacy_preset(json).unwrap();
        // Spot-check 10 fields
        assert!((state.block1.ch1_rotate - 0.0).abs() < 0.001);
        assert!((state.block1.fb1_mix_amount - 0.0).abs() < 0.001);
        assert_eq!(state.block1.fb1_delay_time, 18);
        assert!((state.block1.ch2_mix_amount - 0.03).abs() < 0.001);
        assert!((state.block2.fb2_mix_amount - 0.0).abs() < 0.001);
        assert_eq!(state.block2.fb2_delay_time, 1);
        assert_eq!(state.block3.matrix_mix_type, 0);
        assert!((state.block3.final_mix_amount - 0.61).abs() < 0.001);
        assert!((state.block3.block1_rotate - 0.0).abs() < 0.001);
        assert!((state.block1.ch1_z_displace - 1.0).abs() < 0.001);
    }
}
