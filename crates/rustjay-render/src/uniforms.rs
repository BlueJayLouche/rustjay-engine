//! HSB uniform buffer types for the GPU shader.

use rustjay_core::HsbParams;

/// GPU-side representation of HSB color adjustment parameters.
/// Padded to 16 bytes for uniform buffer alignment.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HsbUniforms {
    /// [hue_shift, saturation, brightness, _padding]
    pub values: [f32; 4],
}

impl From<&HsbParams> for HsbUniforms {
    fn from(params: &HsbParams) -> Self {
        Self {
            values: [params.hue_shift, params.saturation, params.brightness, 0.0],
        }
    }
}

impl HsbUniforms {
    pub fn identity() -> Self {
        Self {
            values: [0.0, 1.0, 1.0, 0.0],
        }
    }
}
