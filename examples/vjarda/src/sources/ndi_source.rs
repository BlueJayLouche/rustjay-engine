//! NDI source — receives frames from an NDI stream and uploads to GPU texture.
//!
//! Wraps `rustjay_io::NdiReceiver` as an `EffectInstance` for use in a deck.

use rustjay_core::{EffectInput, EffectInstance, EngineState, RenderCtx, RenderTarget};
use rustjay_mixer::BlitPipeline;

/// Renders live NDI frames to the target.
pub struct NdiSource {
    receiver: rustjay_io::NdiReceiver,
    source_name: String,
    started: bool,
    pipeline: BlitPipeline,
    texture: Option<wgpu::Texture>,
    view: Option<wgpu::TextureView>,
    width: u32,
    height: u32,
}

impl NdiSource {
    pub fn new(device: &wgpu::Device, source_name: impl Into<String>) -> Self {
        let source_name = source_name.into();
        let receiver = rustjay_io::NdiReceiver::new(&source_name);
        let pipeline = BlitPipeline::new(device, rustjay_core::working_format());
        Self {
            receiver,
            source_name,
            started: false,
            pipeline,
            texture: None,
            view: None,
            width: 1920,
            height: 1080,
        }
    }

    fn ensure_texture(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.texture.is_none() || self.width != width || self.height != height {
            self.width = width;
            self.height = height;
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("NdiSource Texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.view = Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
            self.texture = Some(texture);
        }
    }
}

impl EffectInstance for NdiSource {
    fn prepare(&mut self, _engine: &EngineState, _device: &wgpu::Device, _queue: &wgpu::Queue) {
        if !self.started {
            if let Err(e) = self.receiver.start() {
                log::warn!("[NdiSource] Failed to start receiver: {}", e);
            } else {
                self.started = true;
                log::info!("[NdiSource] Started receiver for '{}'", self.source_name);
            }
        }
    }

    fn render_to(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        _inputs: &[EffectInput<'_>],
        target: RenderTarget<'_>,
        _engine: &EngineState,
    ) {
        if !self.started {
            return;
        }

        if let Some(frame) = self.receiver.get_latest_frame() {
            self.ensure_texture(ctx.device, frame.width, frame.height);
            if let Some(ref texture) = self.texture {
                ctx.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &frame.data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(frame.width * 4),
                        rows_per_image: Some(frame.height),
                    },
                    wgpu::Extent3d {
                        width: frame.width,
                        height: frame.height,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }

        if let Some(ref view) = self.view {
            self.pipeline.blit(
                ctx.device,
                ctx.encoder,
                view,
                target.view,
                ctx.vertex_buffer,
            );
        }
    }
}
