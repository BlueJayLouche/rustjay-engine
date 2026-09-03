//! Spout source — receives frames from a Spout sender (Windows).
//!
//! The Windows counterpart to [`SyphonSource`](super::SyphonSource). Spout input
//! is a CPU path (`rustjay-io` reads BGRA pixels from the sender's shared D3D11
//! texture), so unlike Syphon's zero-copy texture blit we upload the bytes to a
//! `Bgra8Unorm` texture each frame and blit that to the deck target.

use rustjay_core::{EffectInput, EffectInstance, EngineState, RenderCtx, RenderTarget};
use rustjay_io::SpoutInputReceiver;
use rustjay_mixer::BlitPipeline;

/// Renders live Spout frames to the target.
pub struct SpoutSource {
    receiver: SpoutInputReceiver,
    sender_name: String,
    connected: bool,
    pipeline: BlitPipeline,
    texture: Option<wgpu::Texture>,
    view: Option<wgpu::TextureView>,
    width: u32,
    height: u32,
}

impl SpoutSource {
    pub fn new(device: &wgpu::Device, sender_name: impl Into<String>) -> anyhow::Result<Self> {
        Ok(Self {
            receiver: SpoutInputReceiver::new()?,
            sender_name: sender_name.into(),
            connected: false,
            pipeline: BlitPipeline::new(device, rustjay_core::working_format()),
            texture: None,
            view: None,
            width: 0,
            height: 0,
        })
    }

    fn ensure_texture(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.texture.is_some() && self.width == width && self.height == height {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Spout Source Texture"),
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
        self.width = width;
        self.height = height;
    }
}

impl EffectInstance for SpoutSource {
    fn prepare(&mut self, _engine: &EngineState, _device: &wgpu::Device, _queue: &wgpu::Queue) {
        if !self.connected {
            match self.receiver.connect(&self.sender_name) {
                Ok(()) => {
                    self.connected = true;
                    log::info!("[SpoutSource] Connected to '{}'", self.sender_name);
                }
                Err(e) => log::warn!("[SpoutSource] Failed to connect: {}", e),
            }
        }
        if self.connected {
            self.receiver.try_receive_texture();
        }
    }

    fn render_to(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        _inputs: &[EffectInput<'_>],
        target: RenderTarget<'_>,
        _engine: &EngineState,
    ) {
        if !self.connected {
            return;
        }

        // Upload the latest BGRA frame, if any, into our sampleable texture.
        // ensure_texture() before borrowing pixels() to avoid an aliasing borrow.
        let (w, h) = self.receiver.resolution();
        let have_frame = self
            .receiver
            .pixels()
            .map(|p| w > 0 && h > 0 && p.len() == (w * h * 4) as usize)
            .unwrap_or(false);
        if have_frame {
            self.ensure_texture(ctx.device, w, h);
            if let (Some(texture), Some(pixels)) = (self.texture.as_ref(), self.receiver.pixels()) {
                ctx.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    pixels,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w * 4),
                        rows_per_image: Some(h),
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }

        // texture persists between frames, so we keep showing the last frame
        // even when no new one arrived this tick.
        if let Some(ref view) = self.view {
            self.pipeline
                .blit(ctx.device, ctx.encoder, view, target.view, ctx.vertex_buffer);
        }
    }
}
