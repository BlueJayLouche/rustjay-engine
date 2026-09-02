//! Syphon source — receives frames from a Syphon server (macOS).
//!
//! Wraps `rustjay_io::SyphonInputReceiver` as an `EffectInstance` for use in a deck.

use rustjay_core::{EffectInput, EffectInstance, EngineState, RenderCtx, RenderTarget};
use rustjay_mixer::BlitPipeline;

/// Renders live Syphon frames to the target.
pub struct SyphonSource {
    receiver: rustjay_io::SyphonInputReceiver,
    server_name: String,
    server_uuid: String,
    initialized: bool,
    pipeline: BlitPipeline,
}

impl SyphonSource {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        server_name: impl Into<String>,
        server_uuid: impl Into<String>,
    ) -> Self {
        let server_name = server_name.into();
        let server_uuid = server_uuid.into();
        let mut receiver = rustjay_io::SyphonInputReceiver::new();
        receiver.initialize(device, queue);
        let pipeline = BlitPipeline::new(device, rustjay_core::working_format());
        Self {
            receiver,
            server_name,
            server_uuid,
            initialized: false,
            pipeline,
        }
    }
}

impl EffectInstance for SyphonSource {
    fn prepare(&mut self, _engine: &EngineState, device: &wgpu::Device, queue: &wgpu::Queue) {
        if !self.initialized {
            match self
                .receiver
                .connect_by_uuid(&self.server_uuid, &self.server_name)
            { Err(e) => {
                log::warn!("[SyphonSource] Failed to connect: {}", e);
            } _ => {
                self.initialized = true;
                log::info!(
                    "[SyphonSource] Connected to '{}'",
                    self.server_name
                );
            }}
        }
        self.receiver.try_receive_texture(device, queue);
    }

    fn render_to(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        _inputs: &[EffectInput<'_>],
        target: RenderTarget<'_>,
        _engine: &EngineState,
    ) {
        if !self.initialized {
            return;
        }

        if let Some(tex) = self.receiver.output_texture() {
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            self.pipeline.blit(
                ctx.device,
                ctx.encoder,
                &view,
                target.view,
                ctx.vertex_buffer,
            );
        }
    }
}
