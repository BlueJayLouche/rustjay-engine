//! `MixerPlugin` — wraps a [`Mixer`] as an [`EffectPlugin`] so it can be the
//! engine root (`run(MixerPlugin)`).
//!
//! The plugin provides a dummy WGSL shader (required by `PluginRenderer::new`)
//! and a custom [`EffectPlugin::render`] hook that builds a [`RenderCtx`] and
//! drives the mixer's [`EffectInstance::render_to`] path. This needs **no**
//! engine changes — it works through the existing custom-render-hook API.

use rustjay_core::{EffectInput, EffectInstance, EffectPlugin, EngineState, ParameterDescriptor, RenderCtx, RenderTarget};
use crate::Mixer;

/// An [`EffectPlugin`] adapter that turns a [`Mixer`] into the engine root.
///
/// Construct with [`MixerPlugin::new`] and pass to `WgpuEngine::run`:
///
/// ```ignore
/// let mixer = Mixer::new();
/// // ... add channels ...
/// run(MixerPlugin::new(mixer));
/// ```
pub struct MixerPlugin {
    mixer: std::sync::Mutex<Mixer>,
}

impl MixerPlugin {
    /// Wrap an existing mixer.
    pub fn new(mixer: Mixer) -> Self {
        Self { mixer: std::sync::Mutex::new(mixer) }
    }

    /// Lock and return mutable access to the underlying mixer.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Mixer> {
        self.mixer.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Dummy uniform type — never uploaded because `render()` returns `true`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DummyUniforms {
    _pad: [f32; 4],
}

impl EffectPlugin for MixerPlugin {
    type State = ();
    type Uniforms = DummyUniforms;

    /// Minimal valid WGSL so `PluginRenderer::new` can create a pipeline.
    /// The default render path is never reached because [`render`] returns `true`.
    fn shader_source(&self) -> &'static str {
        r#"
        @vertex
        fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> @builtin(position) vec4<f32> {
            return vec4<f32>(position, 0.0, 1.0);
        }
        @fragment
        fn fs_main() -> @location(0) vec4<f32> {
            return vec4<f32>(0.0, 0.0, 0.0, 1.0);
        }
        "#
    }

    fn build_uniforms(&self, _app_state: &(), _engine: &EngineState) -> DummyUniforms {
        DummyUniforms { _pad: [0.0; 4] }
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        self.lock().parameters()
    }

    /// Custom render hook: the engine skips its default pass and lets the mixer
    /// drive all channel rendering, compositing, and the master chain itself.
    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        input_view: Option<&wgpu::TextureView>,
        input_sampler: Option<&wgpu::Sampler>,
        render_target_view: &wgpu::TextureView,
        _app_state: &mut (),
        engine_state: &EngineState,
        vertex_buffer: &wgpu::Buffer,
        input_texture: Option<&wgpu::Texture>,
    ) -> bool {
        let mut ctx = RenderCtx {
            device,
            queue,
            encoder,
            vertex_buffer,
        };

        let size = [
            engine_state.resolution.internal_width,
            engine_state.resolution.internal_height,
        ];

        // Build the input slice the same way the engine does for single effects.
        let primary = match (input_view, input_sampler) {
            (Some(view), Some(sampler)) => Some(EffectInput {
                view,
                sampler,
                generation: 0,
                texture: input_texture,
            }),
            _ => None,
        };
        let one;
        let inputs: &[EffectInput] = match primary {
            Some(p) => {
                one = [p];
                &one
            }
            None => &[],
        };

        let target = RenderTarget {
            view: render_target_view,
            size,
        };

        let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
        mixer.render_to(&mut ctx, inputs, target, engine_state);
        true
    }
}
