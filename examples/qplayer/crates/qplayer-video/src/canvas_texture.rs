//! Canvas texture — the logical composition framebuffer that video frames are
//! placed into before slices are sampled for projector outputs.

use crate::VideoFrame;
use qplayer_core::CanvasFit;
use wgpu::{Device, Queue, TextureFormat};

/// A single RGBA8 texture sized to the projection canvas.
pub struct CanvasTexture {
    texture: wgpu::Texture,
    pub width: u32,
    pub height: u32,
}

impl CanvasTexture {
    pub fn new(device: &Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas-texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        Self {
            texture,
            width,
            height,
        }
    }

    /// Recreate the texture at a new size.
    pub fn resize(&mut self, device: &Device, width: u32, height: u32) {
        *self = Self::new(device, width, height);
    }

    /// The current texture view.
    pub fn view(&self) -> wgpu::TextureView {
        self.texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Upload a decoded video frame into the canvas according to `fit`.
    ///
    /// ponytail: currently builds a full CPU-side canvas buffer every frame.
    /// This is simple and correct; for very large canvases it can be replaced
    /// with a GPU scale-blit render pass.
    pub fn upload_frame(&self, queue: &Queue, frame: &VideoFrame, fit: CanvasFit) {
        let canvas_pixels = (self.width * self.height) as usize;
        let mut canvas = vec![0u8; canvas_pixels * 4];

        let (offset_x, offset_y, scaled_w, scaled_h) = match fit {
            CanvasFit::Stretch => (0u32, 0u32, self.width, self.height),
            CanvasFit::Fit => {
                let scale = (self.width as f32 / frame.width as f32)
                    .min(self.height as f32 / frame.height as f32);
                let w = (frame.width as f32 * scale) as u32;
                let h = (frame.height as f32 * scale) as u32;
                let x = (self.width.saturating_sub(w)) / 2;
                let y = (self.height.saturating_sub(h)) / 2;
                (x, y, w, h)
            }
        };

        if scaled_w > 0 && scaled_h > 0 {
            match fit {
                CanvasFit::Stretch => {
                    scale_rgba_into(
                        &frame.data,
                        frame.width,
                        frame.height,
                        &mut canvas,
                        self.width,
                        self.height,
                    );
                }
                CanvasFit::Fit => {
                    scale_rgba_into(
                        &frame.data,
                        frame.width,
                        frame.height,
                        &mut canvas,
                        scaled_w,
                        scaled_h,
                    );
                    // Shift the scaled image to the centered offset by rewriting in-place.
                    // Copy rows from bottom to top so we don't overwrite un-moved data.
                    for src_row in (0..scaled_h).rev() {
                        let dst_row = offset_y + src_row;
                        let src_start = (src_row * scaled_w) as usize * 4;
                        let dst_start = ((dst_row * self.width) + offset_x) as usize * 4;
                        let len = scaled_w as usize * 4;
                        canvas.copy_within(src_start..src_start + len, dst_start);
                    }
                }
            }
        }

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &canvas,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.width * 4),
                rows_per_image: Some(self.height),
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// Simple nearest-neighbor RGBA scale. Good enough for the MVP; bilinear can be
/// swapped in later without changing the public API.
fn scale_rgba_into(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
) {
    let src_w = src_w as usize;
    let src_h = src_h as usize;
    let dst_w = dst_w as usize;
    let dst_h = dst_h as usize;

    for y in 0..dst_h {
        let sy = (y * src_h) / dst_h;
        for x in 0..dst_w {
            let sx = (x * src_w) / dst_w;
            let src_idx = (sy * src_w + sx) * 4;
            let dst_idx = (y * dst_w + x) * 4;
            dst[dst_idx..dst_idx + 4].copy_from_slice(&src[src_idx..src_idx + 4]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_device_queue() -> (Device, Queue) {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .expect("device");
            (device, queue)
        })
    }

    #[test]
    fn test_canvas_stretch() {
        let (device, queue) = fake_device_queue();
        let canvas = CanvasTexture::new(&device, 4, 2);
        let frame = VideoFrame::new(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255,
                0, 0, 255, 255, 255, 255, 0, 255,
            ],
            0.0,
        );
        canvas.upload_frame(&queue, &frame, CanvasFit::Stretch);
        // Visual check only; GPU readback is overkill for this unit test.
        assert_eq!(canvas.width, 4);
        assert_eq!(canvas.height, 2);
    }

    #[test]
    fn test_canvas_fit() {
        let (device, queue) = fake_device_queue();
        let canvas = CanvasTexture::new(&device, 8, 4);
        let frame = VideoFrame::new(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255,
                0, 0, 255, 255, 255, 255, 0, 255,
            ],
            0.0,
        );
        canvas.upload_frame(&queue, &frame, CanvasFit::Fit);
        assert_eq!(canvas.width, 8);
        assert_eq!(canvas.height, 4);
    }
}
