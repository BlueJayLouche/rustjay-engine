//! # `EffectInstance` — object-safe, nestable effect rendering
//!
//! **Status: B0 stub (Phase B keystone).** See `PHASE_B_ROADMAP.md` §2.
//!
//! [`EffectPlugin`](crate::EffectPlugin) describes *one* effect with a single
//! `State`, a single `Uniforms` type, and a single shader. That generic, static
//! shape is perfect for a standalone app (`delta`, `flux`, …) but it cannot be
//! stored in a `Vec` or nested: a mixer needs to hold *N* independent effects,
//! each potentially a different concrete type.
//!
//! [`EffectInstance`] is the **object-safe** counterpart. It erases the
//! associated types so any effect can be boxed (`Box<dyn EffectInstance>`),
//! collected, and composited. The concrete `PluginRenderer<P>` in
//! `rustjay-render` will implement this trait (task **B0.2**), so wrapping an
//! existing plugin is free.
//!
//! ## Why this lives in `rustjay-core`
//!
//! `rustjay-core` is the shared vocabulary crate with **no internal workspace
//! dependencies**. It therefore cannot reference `rustjay-render`'s
//! `InputTexture` / `Texture` wrappers. The trait deliberately speaks in raw
//! `wgpu` primitives ([`wgpu::TextureView`], [`wgpu::Sampler`]) — the neutral
//! types both the render crate and the (future) mixer crate already agree on.
//!
//! ## Object safety
//!
//! Every method is non-generic and the trait has no associated types, so
//! `dyn EffectInstance` is valid. Keep it that way: do **not** add generic type
//! parameters to trait methods, or `Box<dyn EffectInstance>` stops compiling.
//!
//! ## Not yet wired in
//!
//! This module compiles but nothing constructs or drives an `EffectInstance`
//! yet. Wiring is the rest of B0:
//! - **B0.2** — `impl EffectInstance for PluginRenderer<P>` in `rustjay-render`.
//! - **B0.3** — the engine app loop renders the root effect through
//!   `&mut dyn EffectInstance` instead of the concrete generic.

use crate::{params::ParameterDescriptor, state::EngineState};

/// One input texture handed to an effect for the current frame.
///
/// Mirrors what `rustjay-render`'s `InputTexture` exposes, but as borrowed raw
/// `wgpu` handles so the trait stays free of render-crate types. `generation`
/// is a monotonic counter the implementor can use to invalidate cached bind
/// groups (see the `cached_texture_gen` pattern in `PluginRenderer::render`).
#[derive(Clone, Copy)]
pub struct EffectInput<'a> {
    /// The texture view to sample from.
    pub view: &'a wgpu::TextureView,
    /// The sampler paired with `view`.
    pub sampler: &'a wgpu::Sampler,
    /// Bumped whenever `view` is reallocated (resolution change, new source).
    /// Equal across frames means the bind group may be reused.
    pub generation: u64,
    /// The raw texture backing `view`, when available.
    ///
    /// A [`wgpu::TextureView`] cannot be turned back into its [`wgpu::Texture`],
    /// but effects with a custom render pass (ring buffers, history copies,
    /// feedback) need the texture itself — the `EffectPlugin::render` hook
    /// receives it as `Option<&wgpu::Texture>`. Carry it here when the caller
    /// has it; pass `None` for synthetic/derived views (e.g. an intermediate
    /// pass output) where no standalone texture handle is meaningful.
    pub texture: Option<&'a wgpu::Texture>,
}

/// The destination an effect renders into.
///
/// Bundles the target view with its pixel dimensions. Multi-pass effects need
/// the size to allocate intermediate textures matching the output; a bare
/// [`wgpu::TextureView`] cannot report its own dimensions.
#[derive(Clone, Copy)]
pub struct RenderTarget<'a> {
    /// The view to render the final frame into.
    pub view: &'a wgpu::TextureView,
    /// `[width, height]` of `view` in pixels.
    pub size: [u32; 2],
}

/// Per-frame GPU context threaded through [`EffectInstance::render_to`].
///
/// Bundles the handles the concrete renderer needs (`device`/`queue` to create
/// bind groups on demand, `encoder` to record passes, `vertex_buffer` for the
/// shared full-screen quad) so the trait method keeps a manageable signature.
pub struct RenderCtx<'a> {
    /// Device, for on-demand bind group / pipeline creation.
    pub device: &'a wgpu::Device,
    /// Queue, for buffer writes outside the encoder.
    pub queue: &'a wgpu::Queue,
    /// Command encoder the effect records its pass(es) into.
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// The engine's shared full-screen quad vertex buffer.
    pub vertex_buffer: &'a wgpu::Buffer,
}

/// An effect that can be rendered, boxed, and nested.
///
/// Implementors own their own state and uniforms internally; the engine drives
/// them only through this erased interface. The canonical implementor will be
/// `rustjay-render`'s `PluginRenderer<P>` (B0.2), which adapts any
/// [`EffectPlugin`](crate::EffectPlugin) into an `EffectInstance`.
///
/// ```ignore
/// // Future shape (rustjay-mixer, B3):
/// struct Channel {
///     effect: Box<dyn EffectInstance>,
///     opacity: f32,
///     blend: BlendMode,
/// }
///
/// struct Mixer { channels: Vec<Channel>, /* crossfader, master chain… */ }
///
/// // A mixer is itself an EffectInstance, so it composes and projects like any effect:
/// impl EffectInstance for Mixer { /* composite channels → target */ }
/// ```
pub trait EffectInstance: Send + 'static {
    /// Human-readable label for debugging, profiling, and GUI grouping.
    fn label(&self) -> &str {
        "effect"
    }

    /// Declare the parameters this effect exposes (LFO / audio / OSC / MIDI / Web
    /// targets). Erased equivalent of [`EffectPlugin::parameters`](crate::EffectPlugin::parameters).
    /// Default: none.
    fn parameters(&self) -> Vec<ParameterDescriptor> {
        Vec::new()
    }

    /// Per-frame hook before rendering, for GPU resources that aren't uniforms
    /// (ping-pong textures, ring buffers, mesh rebuilds). Erased equivalent of
    /// [`EffectPlugin::prepare`](crate::EffectPlugin::prepare). Default: no-op.
    fn prepare(
        &mut self,
        _engine: &EngineState,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
    ) {
    }

    /// Set the parameter prefix used when this effect looks up engine params.
    ///
    /// When non-empty, the effect's `render_to` temporarily sets this prefix
    /// on the engine state so nested plugins can use bare IDs (e.g. `"red"`)
    /// while the engine stores them under fully-qualified names (e.g. `"ch_a_red"`).
    ///
    /// Default: no-op. Implementors that wrap `EffectPlugin`s (e.g. `EffectNode`)
    /// should forward this to their inner renderer.
    fn set_param_prefix(&mut self, _prefix: &str) {}

    /// Render this effect into `target`, sampling from `inputs`.
    ///
    /// `inputs` is ordered: `inputs[0]` is the primary video input; additional
    /// slots (second input for mixers, feedback) follow. An effect that needs
    /// fewer inputs than provided ignores the tail; one that needs more than is
    /// provided should render a sensible fallback rather than panic.
    ///
    /// Replaces the concrete `PluginRenderer::render(encoder, device, queue,
    /// input_texture, feedback_texture, render_target, …)` signature with an
    /// erased, slice-based one so callers can hold `&mut dyn EffectInstance`.
    fn render_to(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        inputs: &[EffectInput<'_>],
        target: RenderTarget<'_>,
        engine: &EngineState,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trivial implementor proving the trait is object-safe and boxable.
    /// (It records nothing — it only has to *compile* as `dyn`.)
    struct NoopEffect;

    impl EffectInstance for NoopEffect {
        fn label(&self) -> &str {
            "noop"
        }
        fn render_to(
            &mut self,
            _ctx: &mut RenderCtx<'_>,
            _inputs: &[EffectInput<'_>],
            _target: RenderTarget<'_>,
            _engine: &EngineState,
        ) {
        }
    }

    #[test]
    fn effect_instance_is_object_safe() {
        let effects: Vec<Box<dyn EffectInstance>> = vec![Box::new(NoopEffect)];
        assert_eq!(effects[0].label(), "noop");
        assert!(effects[0].parameters().is_empty());
    }
}
