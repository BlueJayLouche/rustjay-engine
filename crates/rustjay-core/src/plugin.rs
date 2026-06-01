//! # EffectPlugin trait
//!
//! Core abstraction that lets app authors plug their own shader, uniforms,
//! and GPU resources into the engine.

use crate::{EngineState, params::ParameterDescriptor, state::GuiTab};

/// Describes a mesh grid for vertex-shader effects.
///
/// When a plugin returns `Some(MeshDescriptor)`, the engine generates a
/// `cols × rows` indexed grid instead of the default fullscreen quad.
/// Changing the descriptor triggers a mesh rebuild.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MeshDescriptor {
    /// Number of columns in the mesh grid.
    pub cols: u32,
    /// Number of rows in the mesh grid.
    pub rows: u32,
    /// Primitive topology used when rendering the mesh.
    pub topology: MeshTopology,
}

/// Primitive topology for mesh rendering.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MeshTopology {
    /// Horizontal scanlines (`LineList`), classic Rutt-Etra wire look.
    Scanlines,
    /// Solid surface (`TriangleList`), terrain-style displacement.
    Triangles,
    /// Wireframe outline (`TriangleList` + `PolygonMode::Line`).
    /// Gives a classic wire-frame mesh look distinct from scanlines.
    Wireframe,
    /// Point-cloud (`PointList`). Each vertex renders as a single point.
    /// Produces a particle-cloud displacement effect.
    Points,
}

/// Describes a linear multi-pass render pipeline.
///
/// Passes execute in declaration order. Each pass reads from a single
/// input source and writes to either an intermediate texture or the
/// final render target.
///
/// The last pass always writes to the render target; all preceding passes
/// write to intermediate textures that the engine manages automatically.
#[derive(Clone, Debug, Default)]
pub struct RenderGraph {
    /// Ordered list of render passes.
    pub passes: Vec<Pass>,
    /// If true, the engine maintains a feedback texture containing the
    /// previous frame's output and binds it at `@group(0) @binding(2/3)`.
    pub feedback: bool,
}

impl RenderGraph {
    /// Create a new empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a pass to the graph.
    pub fn with_pass(mut self, pass: Pass) -> Self {
        self.passes.push(pass);
        self
    }

    /// Enable the feedback texture.
    pub fn with_feedback(mut self) -> Self {
        self.feedback = true;
        self
    }
}

/// One fullscreen pass inside a [`RenderGraph`].
#[derive(Clone, Debug)]
pub struct Pass {
    /// Human-readable label for debugging and profiling.
    pub label: &'static str,
    /// WGSL source. Must declare `vs_main` and `fs_main`.
    pub shader: &'static str,
    /// Which texture this pass samples from.
    pub input: PassInput,
}

/// Input source for a render-graph pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassInput {
    /// The engine's live video input (webcam / NDI / Syphon / etc).
    EngineInput,
    /// The output of the previous pass in the graph.
    PreviousPass,
    /// The previous frame's feedback texture.
    Feedback,
}

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

    /// Declare all parameters this effect exposes.
    ///
    /// The engine uses these descriptors to auto-generate UI controls,
    /// LFO targets, audio routing targets, and OSC/MIDI/Web mappings.
    ///
    /// Default is empty — effects that don't declare parameters continue
    /// to work by reading directly from `EngineState` (backward compatible).
    fn parameters(&self) -> Vec<ParameterDescriptor> {
        vec![]
    }

    /// Declare which built-in tabs to hide.
    ///
    /// For example, delta hides `GuiTab::Color` since it has no HSB parameters.
    /// Default is empty — all tabs are visible.
    fn hidden_tabs(&self) -> Vec<GuiTab> {
        vec![]
    }

    /// App name used for per-app config file isolation (`~/.config/rustjay/<name>.json`).
    fn app_name(&self) -> &str {
        "rustjay"
    }

    /// Number of input slots this effect actually samples (1 or 2).
    ///
    /// The engine uses this to skip the per-frame GPU upload of input slots the
    /// effect never reads — uploading a full-resolution frame costs a CPU memmove
    /// into wgpu's staging buffer, which matters on CPU-bound targets (Pi/llvmpipe).
    ///
    /// Default is `1`. Override and return `2` if the effect samples
    /// `EngineState::second_input_view` (e.g. a two-channel mixer/blend effect).
    fn input_count(&self) -> u32 {
        1
    }

    /// Initial app state. Override to set non-Default starting values.
    fn default_state(&self) -> Self::State {
        Self::State::default()
    }

    /// Serialize plugin state for preset storage.
    /// Return `Some(json_string)` to include plugin-specific data in the preset.
    fn serialize_preset_state(&self, _state: &Self::State) -> Option<String> {
        None
    }

    /// Deserialize plugin state from preset storage.
    /// Called when a preset containing `plugin_state` is loaded.
    fn deserialize_preset_state(&self, _data: &str, _state: &mut Self::State) {}

    /// Called after a preset has been applied to the engine state.
    /// Use this to sync restored plugin state back to the engine.
    fn on_preset_applied(&self, _state: &mut Self::State, _engine: &mut EngineState) {}

    /// Returns true if the plugin's parameter list has changed since the last frame.
    /// The engine re-queries `parameters()` and refreshes `EngineState` when this is true.
    /// Default: always false (static parameter lists).
    fn parameters_dirty(&self) -> bool { false }

    /// Called by the engine immediately after it reads `parameters()` in response to
    /// `parameters_dirty()` returning true. Reset the dirty flag here.
    fn clear_parameters_dirty(&mut self) {}

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

    /// Optional multi-pass configuration.
    ///
    /// When `Some`, the engine executes the [`RenderGraph`] instead of the
    /// default single-pass pipeline. Each pass gets its own shader, and
    /// intermediate textures are managed automatically.
    ///
    /// The texture bind group layout for graph passes includes a **feedback**
    /// texture at `@group(0) @binding(2)` / `@binding(3)` when
    /// `graph.feedback` is `true`. Shaders that don't use feedback simply
    /// omit those bindings.
    fn render_graph(&self) -> Option<RenderGraph> {
        None
    }

    /// Build uniforms for a specific pass in the render graph.
    ///
    /// `pass_index` corresponds to `render_graph().passes[pass_index]`.
    /// The default implementation delegates to [`build_uniforms`](Self::build_uniforms),
    /// so single-pass plugins work unchanged and simple multi-pass effects
    /// can reuse the same uniform block for every pass.
    fn build_pass_uniforms(
        &self,
        _pass_index: usize,
        app_state: &Self::State,
        engine: &EngineState,
    ) -> Self::Uniforms {
        self.build_uniforms(app_state, engine)
    }

    /// Optional mesh descriptor. When `Some`, the engine generates a
    /// `cols × rows` indexed grid instead of the fullscreen quad.
    ///
    /// Existing plugins return `None` and are completely unaffected.
    fn mesh_descriptor(&self, _state: &Self::State) -> Option<MeshDescriptor> {
        None
    }

    /// When `true`, texture and sampler bind group entries are given
    /// `VERTEX | FRAGMENT` visibility so `vs_main` can sample the video
    /// texture. Required for displacement effects.
    ///
    /// Default is `false` — only the fragment stage can sample textures.
    fn vertex_reads_texture(&self) -> bool {
        false
    }

    /// Optional compute shader that modifies the mesh vertex buffer on the
    /// GPU before the render pass each frame.
    ///
    /// When `Some`, the engine creates a compute pipeline and dispatches it
    /// with enough workgroups to cover all vertices in the mesh. The compute
    /// shader has access to:
    ///
    /// - `@group(0) @binding(0)` — the app's uniform buffer
    /// - `@group(1) @binding(0)` — the vertex storage buffer (`array<Vertex>`)
    ///
    /// The vertex buffer is created with `STORAGE | VERTEX` usage so the
    /// compute shader can write to it and the render pass can read from it.
    ///
    /// Workgroup size must be `@workgroup_size(256, 1, 1)` — the engine
    /// dispatches 1D groups of 256 threads sized to cover all vertices.
    /// The mesh dimensions should be passed through the app's uniform struct
    /// if the compute shader needs them.
    fn compute_shader(&self) -> Option<&'static str> {
        None
    }

    /// Optional custom render pass. If this returns `true`, the engine skips
    /// its default render pass and assumes the plugin has written to the
    /// render target itself.
    ///
    /// Use this when the effect needs extra bind groups, multiple passes,
    /// or compute shaders that the default single-pass pipeline can't express.
    ///
    /// `input_texture` is the raw wgpu texture backing the video input,
    /// useful for effects that need to copy frames into a history ring buffer.
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
        _input_texture: Option<&wgpu::Texture>,
    ) -> bool {
        false
    }
}
