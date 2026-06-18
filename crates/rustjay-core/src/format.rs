//! Working render-target format — optional high-precision mode for smooth gradients.
//!
//! All intermediate (non-surface, non-input-upload) render targets and the
//! pipelines that draw into them use [`working_format`].
//!
//! | `RUSTJAY_RT_FORMAT` | format          | result                              |
//! |---------------------|-----------------|-------------------------------------|
//! | (unset, default)    | `Bgra8Unorm`    | 8-bit — faint gradient banding      |
//! | `f16`               | `Rgba16Float`   | 16-bit float — banding gone, 2x bw  |
//!
//! `f16` costs ~2x intermediate bandwidth, so it's opt-in (leave unset on Pi-class
//! GPUs). sRGB is deliberately *not* an option: engine content is display-referred,
//! so an sRGB buffer/surface washes the image out — see the black-level handling in
//! `app/projection.rs` / `render/renderer.rs`.

use std::sync::OnceLock;

/// The format for all intermediate working render targets. Read once from
/// `RUSTJAY_RT_FORMAT`; defaults to `Bgra8Unorm`. Set `RUSTJAY_RT_FORMAT=f16`
/// for banding-free gradients at ~2x intermediate bandwidth.
pub fn working_format() -> wgpu::TextureFormat {
    static F: OnceLock<wgpu::TextureFormat> = OnceLock::new();
    *F.get_or_init(|| match std::env::var("RUSTJAY_RT_FORMAT").as_deref() {
        Ok("f16") => wgpu::TextureFormat::Rgba16Float,
        _ => wgpu::TextureFormat::Bgra8Unorm,
    })
}
