//! DeckLink input plugin — wraps a [`DecklinkSource`] as an [`EffectPlugin`].
//!
//! Follows the same adapter pattern as [`rustjay_mixer::MixerPlugin`].

// The capture path links the Windows-only DeckLink SDK (see `build.rs`); gate it
// so the example still compiles on macOS/Linux (where it is a no-op plugin).
#[cfg(windows)]
use rustjay_core::{EffectInstance, RenderCtx, RenderTarget};
use rustjay_core::{EffectPlugin, EngineState, RenderHookCtx};

#[cfg(windows)]
mod decklink_source;
#[cfg(windows)]
pub use decklink_source::DecklinkSource;

/// Dummy uniform type — never uploaded because [`render`] returns `true`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DummyUniforms {
    _pad: [f32; 4],
}

/// Root plugin that drives a DeckLink capture source (Windows). On other targets
/// it is a valid but inert plugin so the example builds cross-platform.
pub struct DecklinkApp {
    #[cfg(windows)]
    source: Option<DecklinkSource>,
}

impl DecklinkApp {
    pub fn new() -> Self {
        Self {
            #[cfg(windows)]
            source: None,
        }
    }
}

impl Default for DecklinkApp {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectPlugin for DecklinkApp {
    type State = ();
    type Uniforms = DummyUniforms;

    fn app_name(&self) -> &str {
        "decklink"
    }

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

    fn init(&mut self, device: &wgpu::Device, _queue: &wgpu::Queue) {
        #[cfg(windows)]
        {
            log::info!("DecklinkApp: initializing DeckLink source");
            self.source = Some(DecklinkSource::new(device, 0));
        }
        #[cfg(not(windows))]
        {
            let _ = device;
            log::warn!("DeckLink input is Windows-only; this build renders nothing.");
        }
    }

    fn prepare(
        &mut self,
        _app_state: &mut (),
        engine: &EngineState,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        #[cfg(windows)]
        if let Some(ref mut source) = self.source {
            source.prepare(engine, device, queue);
        }
        #[cfg(not(windows))]
        let _ = (engine, device, queue);
    }

    /// Custom render hook: drives the DeckLink source directly (Windows only).
    fn render(&mut self, ctx: &mut RenderHookCtx<'_>, _app_state: &mut ()) -> bool {
        #[cfg(windows)]
        {
            if let Some(ref mut source) = self.source {
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
                let target = RenderTarget {
                    view: ctx.target_view,
                    size,
                };
                source.render_to(&mut render_ctx, &[], target, ctx.engine_state);
                return true;
            }
        }
        // Non-Windows (or no source): let the engine render its default pass.
        let _ = ctx;
        false
    }
}
