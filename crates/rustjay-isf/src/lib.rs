//! ISF (Interactive Shader Format) support for rustjay-engine.
//!
//! This crate provides:
//! - A GLSL → WGSL transpiler for ISF shaders
//! - [`IsfEffect`], an [`EffectPlugin`] implementation that loads and renders ISF shaders at runtime
//! - Utilities to bridge ISF metadata to rustjay-engine parameter descriptors
//!
//! # Quick start
//! ```ignore
//! use rustjay_isf::IsfEffect;
//!
//! let effect = IsfEffect::from_path(std::path::Path::new("shaders/MyShader.fs"))?;
//! ```

pub mod effect;
pub mod params;
pub mod transpiler;

pub use effect::{IsfEffect, IsfState, IsfUniforms};
pub use params::{isf_inputs_to_default_values, isf_inputs_to_parameters};
pub use transpiler::{generate_wgsl, Transpiled, UniformIndex, MAX_ISF_UNIFORMS};

/// Path to the tiny one-line config file that stores the last-used shader path.
///
/// Apps can use this to persist the current ISF shader across restarts.
pub fn last_shader_config_path() -> std::path::PathBuf {
    let base = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".config").join("rustjay"))
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("isf-last-shader.txt")
}
