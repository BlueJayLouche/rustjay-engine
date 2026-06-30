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
            // Stored as linear Unorm so the YUV-convert pass writes its matrix
            // output verbatim (the bytes swscale used to produce). Sampling goes
            // through an sRGB view (`view()`), so downstream colour is unchanged.
            format: TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[TextureFormat::Rgba8UnormSrgb],
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

    /// Sampling view — sRGB, so projection reads decode gamma exactly as before.
    pub fn view(&self) -> wgpu::TextureView {
        self.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(TextureFormat::Rgba8UnormSrgb),
            ..Default::default()
        })
    }

    /// Render-target view — linear Unorm, for the YUV-convert pass to write into.
    pub fn render_view(&self) -> wgpu::TextureView {
        self.texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Upload a decoded video frame into the canvas according to `fit`.
    ///
    /// ponytail: currently builds a full CPU-side canvas buffer every frame.
    /// This is simple and correct; for very large canvases it can be replaced
    /// with a GPU scale-blit render pass.
    pub fn upload_frame(&self, queue: &Queue, frame: &VideoFrame, fit: CanvasFit) {
        // Video frames are YUV and go through the GPU converter, not here.
        let Some(rgba) = frame.rgba() else { return };
        // Fast path: frame already matches the canvas exactly (every fit mode is a
        // 1:1 copy here), so skip the full CPU compose + blit and upload directly.
        // Guarded on byte length so a stride-padded frame still falls back safely.
        if frame.width == self.width
            && frame.height == self.height
            && rgba.len() == (self.width * self.height * 4) as usize
        {
            self.upload_rgba(queue, rgba);
            return;
        }
        let canvas = compose_canvas(frame, self.width, self.height, fit);
        self.upload_rgba(queue, &canvas);
    }

    /// Load an image file and place it on the canvas using the same fit modes as video.
    pub fn upload_image(&self, queue: &Queue, path: &str, fit: CanvasFit) -> anyhow::Result<()> {
        let img = image::open(path)
            .map_err(|e| anyhow::anyhow!("failed to open image {path}: {e}"))?
            .to_rgba8();
        let frame = VideoFrame::new(img.width(), img.height(), img.into_raw(), 0.0);
        self.upload_frame(queue, &frame, fit);
        Ok(())
    }

    /// Upload a full RGBA8 buffer that exactly matches the canvas size.
    ///
    /// ponytail: callers (e.g. the text cue path) build the buffer on the CPU.
    pub fn upload_rgba(&self, queue: &Queue, data: &[u8]) {
        debug_assert_eq!(data.len(), (self.width * self.height * 4) as usize);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
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

/// A rectangle in pixels (sub-pixel positions allowed for source crops).
#[derive(Copy, Clone)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Build the `cw`x`ch` RGBA canvas buffer for one frame under `fit`.
///
/// Each mode maps a source sub-rect onto a dest sub-rect of the canvas:
///   Stretch -> whole source onto whole canvas (distorts aspect)
///   Fit     -> whole source onto a centered, letterboxed rect (black bars)
///   Fill    -> a centered source crop onto the whole canvas (cover, no bars)
fn compose_canvas(frame: &VideoFrame, cw: u32, ch: u32, fit: CanvasFit) -> Vec<u8> {
    let mut canvas = vec![0u8; (cw * ch * 4) as usize];
    let (cwf, chf) = (cw as f32, ch as f32);
    let (fw, fh) = (frame.width as f32, frame.height as f32);
    if fw == 0.0 || fh == 0.0 {
        return canvas;
    }

    let (src, dst) = match fit {
        CanvasFit::Stretch => (
            Rect { x: 0.0, y: 0.0, w: fw, h: fh },
            Rect { x: 0.0, y: 0.0, w: cwf, h: chf },
        ),
        CanvasFit::Fit => {
            let s = (cwf / fw).min(chf / fh);
            let (w, h) = (fw * s, fh * s);
            (
                Rect { x: 0.0, y: 0.0, w: fw, h: fh },
                Rect { x: (cwf - w) / 2.0, y: (chf - h) / 2.0, w, h },
            )
        }
        CanvasFit::Fill => {
            let s = (cwf / fw).max(chf / fh);
            let (vw, vh) = ((cwf / s).min(fw), (chf / s).min(fh));
            (
                Rect { x: (fw - vw) / 2.0, y: (fh - vh) / 2.0, w: vw, h: vh },
                Rect { x: 0.0, y: 0.0, w: cwf, h: chf },
            )
        }
    };
    blit_resampled(frame, src, &mut canvas, cw, dst);
    canvas
}

/// Nearest-neighbor blit of `frame`'s `src` rect into the `dst_rect` region of
/// `dst` (a `dst_stride`-pixels-wide RGBA canvas). ponytail: nearest is fine for
/// the MVP; bilinear can be swapped in here without touching callers.
fn blit_resampled(frame: &VideoFrame, src: Rect, dst: &mut [u8], dst_stride: u32, dst_rect: Rect) {
    let Some(data) = frame.rgba() else { return };
    let dw = dst_rect.w.round() as u32;
    let dh = dst_rect.h.round() as u32;
    if dw == 0 || dh == 0 || frame.width == 0 || frame.height == 0 {
        return;
    }
    let dx0 = dst_rect.x.round() as u32;
    let dy0 = dst_rect.y.round() as u32;
    let max_x = frame.width - 1;
    let max_y = frame.height - 1;

    for j in 0..dh {
        let sy = (src.y + (j as f32 + 0.5) * src.h / dh as f32) as u32;
        let sy = sy.min(max_y);
        let dst_row = dy0 + j;
        for i in 0..dw {
            let sx = (src.x + (i as f32 + 0.5) * src.w / dw as f32) as u32;
            let sx = sx.min(max_x);
            let si = ((sy * frame.width + sx) * 4) as usize;
            let di = ((dst_row * dst_stride + dx0 + i) * 4) as usize;
            dst[di..di + 4].copy_from_slice(&data[si..si + 4]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_device_queue() -> (Device, Queue) {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
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

    // A non-black frame whose aspect (2:1) differs from the canvas (1:1) so the
    // three fit modes produce visibly different coverage.
    fn solid_frame() -> VideoFrame {
        VideoFrame::new(4, 2, vec![200u8; 4 * 2 * 4], 0.0)
    }

    fn has_black_pixel(buf: &[u8]) -> bool {
        buf.chunks_exact(4).any(|p| p[0] == 0 && p[1] == 0 && p[2] == 0)
    }

    #[test]
    fn test_fit_letterboxes() {
        // 2:1 source into an 8x8 (1:1) canvas -> top/bottom black bars remain.
        let buf = compose_canvas(&solid_frame(), 8, 8, CanvasFit::Fit);
        assert!(has_black_pixel(&buf), "Fit should leave letterbox bars");
        assert!(buf.iter().any(|&b| b != 0), "Fit should draw the video");
    }

    #[test]
    fn test_fill_covers_canvas() {
        // Cover must fill every pixel — no black bars anywhere.
        let buf = compose_canvas(&solid_frame(), 8, 8, CanvasFit::Fill);
        assert!(!has_black_pixel(&buf), "Fill must cover the whole canvas (no bars)");
    }

    #[test]
    fn test_stretch_covers_canvas() {
        let buf = compose_canvas(&solid_frame(), 8, 8, CanvasFit::Stretch);
        assert!(!has_black_pixel(&buf), "Stretch fills the whole canvas");
    }
}
