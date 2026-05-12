//! # EffectPlugin trait
//!
//! Core abstraction that lets app authors plug their own shader, uniforms,
//! and GPU resources into the engine.

use crate::EngineState;

/// App-authored effect that the engine renders each frame.
///
/// The engine provides:
/// - Video input texture (bound @group(0) @binding(0..1))
/// - A full-screen quad vertex buffer
/// - The render target to draw into
///
/// The plugin provides:
/// - WGSL shader source
/// - A uniform type (bound @group(1) @binding(0))
/// - GPU resource init / per-frame preparation hooks
pub trait EffectPlugin: Send + Sync + 'static {
    /// App-specific state (parameters, extra textures, etc.)
    type State: Default + Send + Sync + serde::Serialize + serde::de::DeserializeOwned + 'static;

    /// GPU uniform type — must be Pod + Zeroable for bytemuck upload.
    /// Bound at @group(1) @binding(0) in the shader.
    type Uniforms: bytemuck::Pod + bytemuck::Zeroable;

    /// WGSL source for the main effect shader.
    ///
    /// The shader **must** declare:
    /// ```wgsl,ignore
    /// @group(0) @binding(0) var input_tex: texture_2d<f32>;
    /// @group(0) @binding(1) var input_sampler: sampler;
    /// @group(1) @binding(0) var<uniform> my_uniforms: MyUniforms;
    /// ```
    fn shader_source(&self) -> &'static str;

    /// Build uniforms from the current app + engine state, called every frame.
    fn build_uniforms(
        &self,
        app_state: &Self::State,
        engine: &EngineState,
    ) -> Self::Uniforms;

    /// App name used for per-app config file isolation (`~/.config/rustjay/<name>.json`).
    fn app_name(&self) -> &str {
        "rustjay"
    }

    /// Initial app state. Override to set non-Default starting values.
    fn default_state(&self) -> Self::State {
        Self::State::default()
    }

    /// Lifecycle hook: called once when the wgpu device/queue are ready.
    /// Use this to create extra textures, bind groups, pipelines, etc.
    fn init(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue) {}

    /// Lifecycle hook: called every frame before rendering.
    /// Use this to update GPU resources that aren't uniforms (e.g. ping-pong textures).
    fn prepare(
        &mut self,
        _app_state: &mut Self::State,
        _engine: &EngineState,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
    ) {
    }

    /// Optional custom render pass. If this returns `true`, the engine skips
    /// its default render pass and assumes the plugin has written to the
    /// render target itself.
    ///
    /// Use this when the effect needs extra bind groups, multiple passes,
    /// or compute shaders that the default single-pass pipeline can't express.
    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        _encoder: &mut wgpu::CommandEncoder,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _input_view: Option<&wgpu::TextureView>,
        _input_sampler: Option<&wgpu::Sampler>,
        _render_target_view: &wgpu::TextureView,
        _app_state: &mut Self::State,
        _engine_state: &EngineState,
        _vertex_buffer: &wgpu::Buffer,
    ) -> bool {
        false
    }
}
