//! [`EffectInstance`] adapter that bundles a [`PluginRenderer<P>`] with its owned state.

use rustjay_core::{
    EffectInput, EffectInstance, EffectPlugin, EngineState, ParameterDescriptor, RenderCtx,
    RenderTarget,
};

use crate::plugin_renderer::PluginRenderer;

/// Bundles a [`PluginRenderer<P>`] with its owned state, satisfying [`EffectInstance`].
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
