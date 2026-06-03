//! [`EffectInstance`] adapter for [`EffectPlugin`]s (Phase B0.2).
//!
//! See `PHASE_B_ROADMAP.md` §2 and the `EffectInstance` docs in `rustjay-core`.
//!
//! ## Why a wrapper, not `impl EffectInstance for PluginRenderer<P>`
//!
//! An `EffectInstance` is **self-contained**: its `prepare`/`render_to` methods
//! take no `app_state`, because a boxed effect must own everything it needs to
//! render. But `PluginRenderer<P>` does **not** own the plugin's state — today
//! `app_state: P::State` lives in the engine's `App` struct
//! (`rustjay-engine/src/app/mod.rs:178`) and is threaded into
//! `PluginRenderer::render(…, app_state, …)` from the caller.
//!
//! [`EffectNode<P>`] closes that gap: it bundles the `PluginRenderer<P>` with the
//! `P::State` it drives, so the pair satisfies the erased, state-free trait.

use rustjay_core::{
    EffectInput, EffectInstance, EffectPlugin, EngineState, ParameterDescriptor, RenderCtx,
    RenderTarget,
};

use crate::plugin_renderer::PluginRenderer;

/// A boxable, nestable effect: a [`PluginRenderer<P>`] plus the [`P::State`] it
/// renders from, exposed through the object-safe [`EffectInstance`] trait.
///
/// ```ignore
/// // One channel of a mixer (B3):
/// let mut node: Box<dyn EffectInstance> =
///     Box::new(EffectNode::new(MyEffect, "channel A", &device, &queue, &engine));
/// node.prepare(&engine, &device, &queue);
/// node.render_to(&mut ctx, &inputs, &target_view, &engine);
/// ```
///
/// [`P::State`]: rustjay_core::EffectPlugin::State
pub struct EffectNode<P: EffectPlugin> {
    renderer: PluginRenderer<P>,
    state: P::State,
    label: String,
    param_prefix: String,
}

impl<P: EffectPlugin> EffectNode<P> {
    /// Build an effect node, compiling the plugin's pipeline and seeding its
    /// state from [`EffectPlugin::default_state`].
    pub fn new(
        plugin: P,
        label: impl Into<String>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        engine_state: &EngineState,
    ) -> Self {
        let state = plugin.default_state();
        let renderer = PluginRenderer::new(plugin, device, queue, engine_state);
        Self {
            renderer,
            state,
            label: label.into(),
            param_prefix: String::new(),
        }
    }

    /// Borrow the plugin's state (for preset save, GUI binding, etc.).
    pub fn state(&self) -> &P::State {
        &self.state
    }

    /// Mutably borrow the plugin's state.
    pub fn state_mut(&mut self) -> &mut P::State {
        &mut self.state
    }

    /// Borrow the wrapped plugin.
    pub fn plugin(&self) -> &P {
        &self.renderer.plugin
    }

    /// Set the parameter prefix used when this effect looks up engine params.
    ///
    /// When non-empty, [`EffectNode::render_to`] temporarily sets this prefix
    /// on the [`EngineState::param_lookup_prefix`] field so the nested plugin's
    /// `build_uniforms` can use bare IDs (e.g. `"red"`) while the engine stores
    /// the value under the fully-qualified name (e.g. `"ch_a_red"`).
    pub fn set_param_prefix(&mut self, prefix: &str) {
        self.param_prefix = prefix.to_string();
    }
}

impl<P: EffectPlugin> EffectInstance for EffectNode<P> {
    fn set_param_prefix(&mut self, prefix: &str) {
        self.param_prefix = prefix.to_string();
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        self.renderer.plugin.parameters()
    }

    fn prepare(&mut self, engine: &EngineState, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.renderer
            .plugin
            .prepare(&mut self.state, engine, device, queue);
    }

    fn render_to(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        inputs: &[EffectInput<'_>],
        target: RenderTarget<'_>,
        engine: &EngineState,
    ) {
        let old_prefix = engine.param_lookup_prefix.borrow().clone();
        if !self.param_prefix.is_empty() {
            *engine.param_lookup_prefix.borrow_mut() = Some(self.param_prefix.clone());
        }
        self.renderer.render_to_view(
            ctx.encoder,
            ctx.device,
            ctx.queue,
            inputs,
            target.view,
            (target.size[0], target.size[1]),
            &mut self.state,
            engine,
            ctx.vertex_buffer,
        );
        *engine.param_lookup_prefix.borrow_mut() = old_prefix;
    }
}
