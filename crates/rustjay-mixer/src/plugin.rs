//! `MixerPlugin` — wraps a [`Mixer`] as an [`EffectPlugin`] so it can be the
//! engine root (`run(MixerPlugin)`).
//!
//! The plugin provides a dummy WGSL shader (required by `PluginRenderer::new`)
//! and a custom [`EffectPlugin::render`] hook that builds a [`RenderCtx`] and
//! drives the mixer's [`EffectInstance::render_to`] path. This needs **no**
//! engine changes — it works through the existing custom-render-hook API.

use crate::Mixer;
use rustjay_core::{
    EffectInput, EffectInstance, EffectPlugin, EngineState, ParameterDescriptor, RenderCtx,
    RenderHookCtx, RenderTarget,
};

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
        Self {
            mixer: std::sync::Mutex::new(mixer),
        }
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

    /// Serialize the mixer's mix state (crossfader + per-channel settings) into
    /// the engine preset (REQ-10.1). Per-effect parameter *values* are captured
    /// separately by the engine's main preset, since T08 aggregates them as
    /// engine parameters — see [`crate::preset`].
    fn serialize_preset_state(&self, _state: &()) -> Option<String> {
        match self.lock().serialize_state().to_json() {
            Ok(json) => Some(json),
            Err(e) => {
                log::error!("mixer: failed to serialize preset state: {e}");
                None
            }
        }
    }

    /// Restore mix state from a preset, matched to live channels by UUID with
    /// bounded validation (REQ-10.3).
    fn deserialize_preset_state(&self, data: &str, _state: &mut ()) {
        match crate::MixerState::from_json(data) {
            Ok(state) => {
                let (matched, legacy) = self.lock().apply_state(&state);
                if legacy.is_some() {
                    log::warn!("mixer: preset contained v1 modulation state that was discarded. Reload via EngineState to migrate into the unified ModulationEngine.");
                }
                log::info!("mixer: restored {matched} channel(s) from preset");
            }
            Err(e) => log::error!("mixer: rejected preset state: {e}"),
        }
    }

    /// Custom render hook: the engine skips its default pass and lets the mixer
    /// drive all channel rendering, compositing, and the master chain itself.
    fn render(&mut self, ctx: &mut RenderHookCtx<'_>, _app_state: &mut ()) -> bool {
        let mut render_ctx = RenderCtx {
            device: ctx.device,
            queue: ctx.queue,
            encoder: ctx.encoder,
            vertex_buffer: ctx.vertex_buffer,
        };

        let size = [
            ctx.engine_state.resolution.internal_width,
            ctx.engine_state.resolution.internal_height,
        ];

        // Build the input slice the same way the engine does for single effects.
        let primary = ctx.input.map(|i| EffectInput {
            view: i.view,
            sampler: i.sampler,
            generation: i.generation,
            texture: i.texture,
        });
        let one;
        let inputs: &[EffectInput] = match primary {
            Some(p) => {
                one = [p];
                &one
            }
            None => &[],
        };

        let target = RenderTarget {
            view: ctx.target_view,
            size,
        };

        let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
        mixer.render_to(&mut render_ctx, inputs, target, ctx.engine_state);
        true
    }
}
