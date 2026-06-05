//! Headless output — offscreen texture with async GPU→CPU readback.
//!
//! Runs the same `ProjectionStage` pipeline as a windowed output but has no
//! winit window / no surface.  The rendered frame is copied to a mappable
//! `wgpu::Buffer` each frame via `map_async`; a non-blocking poll lets the
//! caller retrieve the latest frame without stalling the render thread.

use crate::stage::ProjectionStage;
use rustjay_core::RenderCtx;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// Readback state shared with the `map_async` callback.
const MAP_PENDING: u8 = 0;
const MAP_READY: u8 = 1;
const MAP_FAILED: u8 = 2;

/// A headless projector output: offscreen texture + stage chain + async readback.
///
/// The output format is hard-coded to `Rgba8Unorm` (linear, non-sRGB) per
/// REQ-08.6.
pub struct HeadlessOutput {
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    stages: Vec<Box<dyn ProjectionStage>>,
    offscreen_texture: wgpu::Texture,
    offscreen_view: wgpu::TextureView,
    ping_textures: Vec<wgpu::Texture>,
    ping_views: Vec<wgpu::TextureView>,
    readback_buffer: wgpu::Buffer,
    /// Tightly-packed RGBA8 pixels from the latest completed readback.
    latest_pixels: Vec<u8>,
    /// Readback status for the in-flight buffer, set by the `map_async` callback
    /// (`MAP_PENDING` / `MAP_READY` / `MAP_FAILED`).
    map_state: Arc<AtomicU8>,
    /// Whether a readback is currently in flight (buffer may be mapped).
    readback_in_flight: bool,
    /// Whether `latest_pixels` contains at least one completed frame.
    has_frame: bool,
    dummy_vb: wgpu::Buffer,
}

impl HeadlessOutput {
    /// Create a new headless output with the given size and stage chain.
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        stages: Vec<Box<dyn ProjectionStage>>,
    ) -> Self {
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let (offscreen_texture, offscreen_view) = create_offscreen(device, width, height, format);
        let (ping_textures, ping_views) =
            create_ping_pong(device, width, height, format, stages.len());
        let readback_buffer = create_readback_buffer(device, width, height);

        let latest_pixels = vec![0; (width * height * 4) as usize];

        let dummy_vb = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Headless Dummy VB"),
            size: 64,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });

        Self {
            width,
            height,
            format,
            stages,
            offscreen_texture,
            offscreen_view,
            ping_textures,
            ping_views,
            readback_buffer,
            latest_pixels,
            map_state: Arc::new(AtomicU8::new(MAP_PENDING)),
            readback_in_flight: false,
            has_frame: false,
            dummy_vb,
        }
    }

    /// Resize the offscreen texture, ping-pong textures, and readback buffer.
    ///
    /// Reallocation happens only when the size actually changes (REQ-08.1).
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width > 0 && height > 0 && (width != self.width || height != self.height) {
            self.width = width;
            self.height = height;

            let (offscreen_texture, offscreen_view) =
                create_offscreen(device, width, height, self.format);
            self.offscreen_texture = offscreen_texture;
            self.offscreen_view = offscreen_view;

            let (ping_textures, ping_views) =
                create_ping_pong(device, width, height, self.format, self.stages.len());
            self.ping_textures = ping_textures;
            self.ping_views = ping_views;

            self.readback_buffer = create_readback_buffer(device, width, height);
            self.latest_pixels.resize((width * height * 4) as usize, 0);
            self.has_frame = false;

            // Abandon any in-flight readback against the old buffer. The old
            // buffer's `map_async` callback still holds a clone of the previous
            // `mapped_flag` Arc; swap in a fresh Arc so that stale callback can
            // never flip the flag we now poll against (which would call
            // `get_mapped_range` on the new, unmapped buffer → panic).
            self.readback_in_flight = false;
            self.map_state = Arc::new(AtomicU8::new(MAP_PENDING));

            for stage in &mut self.stages {
                stage.on_input_changed(device, [width, height]);
            }
        }
    }

    /// Render the source through the stage chain and enqueue a readback.
    ///
    /// If a previous readback is still in flight the render proceeds but the
    /// readback is skipped for this frame — this avoids overlapping
    /// `map_async` calls on the same buffer (no per-frame allocation).
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        input_view: &wgpu::TextureView,
        input_texture: Option<&wgpu::Texture>,
        _input_size: [u32; 2],
    ) {
        // Try to collect a previously-submitted readback first.
        self.poll_readback(device);

        let n = self.stages.len();
        if n == 0 {
            return;
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Headless Stage Chain"),
        });

        for (i, stage) in self.stages.iter_mut().enumerate() {
            let is_first = i == 0;
            let is_last = i == n - 1;

            let in_view: &wgpu::TextureView = if is_first {
                input_view
            } else {
                &self.ping_views[(i - 1) % self.ping_views.len()]
            };
            let in_tex: Option<&wgpu::Texture> = if is_first {
                input_texture
            } else {
                Some(&self.ping_textures[(i - 1) % self.ping_textures.len()])
            };
            let out_view: &wgpu::TextureView = if is_last {
                &self.offscreen_view
            } else {
                &self.ping_views[i % self.ping_views.len()]
            };

            let mut ctx = RenderCtx {
                device,
                queue,
                encoder: &mut encoder,
                vertex_buffer: &self.dummy_vb,
            };

            stage.render(&mut ctx, in_view, in_tex, out_view, [self.width, self.height]);
        }

        // If no readback is in flight, encode the copy and start mapping.
        if !self.readback_in_flight {
            let bytes_per_row = ((self.width * 4).div_ceil(256)) * 256;
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.offscreen_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &self.readback_buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: Some(self.height),
                    },
                },
                wgpu::Extent3d {
                    width: self.width,
                    height: self.height,
                    depth_or_array_layers: 1,
                },
            );

            queue.submit(std::iter::once(encoder.finish()));

            // Fresh state per submission so a callback from a previously-abandoned
            // buffer (e.g. after a resize) can never flip the state we poll here.
            let state = Arc::new(AtomicU8::new(MAP_PENDING));
            self.map_state = Arc::clone(&state);
            self.readback_buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |res| {
                    match res {
                        Ok(()) => state.store(MAP_READY, Ordering::SeqCst),
                        Err(e) => {
                            log::error!("headless readback map_async failed: {e:?}");
                            state.store(MAP_FAILED, Ordering::SeqCst);
                        }
                    }
                });
            self.readback_in_flight = true;
        } else {
            queue.submit(std::iter::once(encoder.finish()));
        }
    }

    /// Poll the device non-blocking and, if the readback has completed, copy
    /// the pixels into `latest_pixels` (tightly packed, padding stripped) and
    /// unmap the buffer.
    ///
    /// Call this regularly (e.g. once per frame) so completed readbacks are
    /// drained and new ones can be submitted.
    pub fn poll_readback(&mut self, device: &wgpu::Device) {
        if !self.readback_in_flight {
            return;
        }

        device.poll(wgpu::PollType::Poll).ok();

        match self.map_state.load(Ordering::SeqCst) {
            MAP_READY => {
                let bytes_per_row = ((self.width * 4).div_ceil(256)) * 256;
                let slice = self.readback_buffer.slice(..);
                let data = slice.get_mapped_range();
                self.latest_pixels.clear();
                self.latest_pixels
                    .reserve((self.width * self.height * 4) as usize);
                for row in 0..self.height {
                    let start = (row * bytes_per_row) as usize;
                    self.latest_pixels
                        .extend_from_slice(&data[start..start + (self.width * 4) as usize]);
                }
                drop(data);
                self.readback_buffer.unmap();
                self.readback_in_flight = false;
                self.has_frame = true;
            }
            MAP_FAILED => {
                // Mapping failed (already logged in the callback). Drop the
                // in-flight state so a fresh readback can be submitted next frame;
                // do NOT call get_mapped_range on an unmapped buffer.
                self.readback_in_flight = false;
            }
            _ => {} // MAP_PENDING — still in flight.
        }
    }

    /// Returns the latest completed frame as tightly-packed RGBA8 bytes,
    /// or `None` if no frame has been read back yet.
    pub fn latest_frame(&self) -> Option<&[u8]> {
        if self.has_frame {
            Some(&self.latest_pixels)
        } else {
            None
        }
    }

    /// Current output size in pixels.
    pub fn size(&self) -> [u32; 2] {
        [self.width, self.height]
    }
}

fn create_offscreen(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Headless Offscreen"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_ping_pong(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    stage_count: usize,
) -> (Vec<wgpu::Texture>, Vec<wgpu::TextureView>) {
    let mut textures = Vec::new();
    let mut views = Vec::new();
    let count = if stage_count > 1 {
        2
    } else if stage_count == 1 {
        1
    } else {
        0
    };
    for i in 0..count {
        let t = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("Headless Ping-Pong {i}")),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let v = t.create_view(&wgpu::TextureViewDescriptor::default());
        textures.push(t);
        views.push(v);
    }
    (textures, views)
}

fn create_readback_buffer(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Buffer {
    let bytes_per_row = ((width * 4).div_ceil(256)) * 256;
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Headless Readback Buffer"),
        size: bytes_per_row as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::IdentityStage;

    #[test]
    fn headless_identity_readback() {
        let (device, queue) = pollster::block_on(crate::test_harness::init_wgpu());

        let (input_tex, input_view) = crate::test_harness::create_checkerboard_texture(&device, &queue);
        let mut output = HeadlessOutput::new(
            &device,
            2,
            2,
            vec![Box::new(IdentityStage::new(&device, wgpu::TextureFormat::Rgba8Unorm))],
        );

        output.render(&device, &queue, &input_view, Some(&input_tex), [2, 2]);

        // Poll until the readback completes (usually immediate on CPU backends).
        let start = std::time::Instant::now();
        while output.latest_frame().is_none() && start.elapsed() < std::time::Duration::from_secs(5)
        {
            output.poll_readback(&device);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        let pixels = output.latest_frame().expect("readback should complete");
        assert_eq!(pixels.len(), 16, "expected 16 bytes for 2×2 RGBA8");

        // Expected: TL white, TR black, BL black, BR white (row-major)
        assert_eq!(&pixels[0..4], &[255, 255, 255, 255]);
        assert_eq!(&pixels[4..8], &[0, 0, 0, 255]);
        assert_eq!(&pixels[8..12], &[0, 0, 0, 255]);
        assert_eq!(&pixels[12..16], &[255, 255, 255, 255]);
    }

    #[test]
    fn headless_resize_reallocates() {
        let (device, queue) = pollster::block_on(crate::test_harness::init_wgpu());

        let (input_tex, input_view) =
            crate::test_harness::create_solid_texture(&device, &queue, 4, 4, [128, 64, 32, 255]);
        let mut output = HeadlessOutput::new(
            &device,
            2,
            2,
            vec![Box::new(IdentityStage::new(&device, wgpu::TextureFormat::Rgba8Unorm))],
        );

        // Render at 2×2
        output.render(&device, &queue, &input_view, Some(&input_tex), [4, 4]);
        let start = std::time::Instant::now();
        while output.latest_frame().is_none() && start.elapsed() < std::time::Duration::from_secs(5)
        {
            output.poll_readback(&device);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(output.size(), [2, 2]);

        // Resize to 4×4
        output.resize(&device, 4, 4);
        assert_eq!(output.size(), [4, 4]);
        assert!(
            output.latest_frame().is_none(),
            "frame should be invalidated after resize"
        );

        // Render again at 4×4
        output.render(&device, &queue, &input_view, Some(&input_tex), [4, 4]);
        let start = std::time::Instant::now();
        while output.latest_frame().is_none() && start.elapsed() < std::time::Duration::from_secs(5)
        {
            output.poll_readback(&device);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        let pixels = output.latest_frame().unwrap();
        assert_eq!(pixels.len(), 4 * 4 * 4);

        // Every pixel should be the solid colour (nearest sampling, same size).
        for i in 0..(4 * 4) {
            let base = i * 4;
            assert_eq!(
                &pixels[base..base + 4],
                &[128, 64, 32, 255],
                "pixel {i} mismatch after resize"
            );
        }
    }

    #[test]
    fn headless_row_padding_handled() {
        let (device, queue) = pollster::block_on(crate::test_harness::init_wgpu());

        // 65×1 forces a padded row: 65*4 = 260, div_ceil(260, 256) = 2, so
        // bytes_per_row = 512 — exercises the un-padding logic.
        let width = 65;
        let height = 1;
        let (input_tex, input_view) =
            crate::test_harness::create_solid_texture(&device, &queue, width, height, [42, 43, 44, 255]);
        let mut output = HeadlessOutput::new(
            &device,
            width,
            height,
            vec![Box::new(IdentityStage::new(&device, wgpu::TextureFormat::Rgba8Unorm))],
        );

        output.render(&device, &queue, &input_view, Some(&input_tex), [width, height]);
        let start = std::time::Instant::now();
        while output.latest_frame().is_none() && start.elapsed() < std::time::Duration::from_secs(5)
        {
            output.poll_readback(&device);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        let pixels = output.latest_frame().unwrap();
        assert_eq!(pixels.len(), (width * height * 4) as usize);
        for i in 0..(width * height) as usize {
            let base = i * 4;
            assert_eq!(&pixels[base..base + 4], &[42, 43, 44, 255]);
        }
    }

    // Regression: resizing while a readback is in flight must not panic on the
    // next poll (previously the stale buffer's callback could flip the shared
    // flag and trigger `get_mapped_range` on the new, unmapped buffer).
    #[test]
    fn headless_resize_while_readback_in_flight() {
        let (device, queue) = pollster::block_on(crate::test_harness::init_wgpu());
        let (input_tex, input_view) =
            crate::test_harness::create_solid_texture(&device, &queue, 4, 4, [10, 20, 30, 255]);
        let mut output = HeadlessOutput::new(
            &device,
            2,
            2,
            vec![Box::new(IdentityStage::new(&device, wgpu::TextureFormat::Rgba8Unorm))],
        );

        // Submit a readback but do NOT poll it to completion — leave it in flight.
        output.render(&device, &queue, &input_view, Some(&input_tex), [4, 4]);

        // Resize mid-flight: abandons the old buffer + its callback's flag.
        output.resize(&device, 4, 4);

        // Poll repeatedly: the abandoned callback may now fire against the old
        // flag, but it must not affect the new buffer. No panic expected.
        for _ in 0..10 {
            output.poll_readback(&device);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        // A fresh render at the new size still reads back correctly.
        output.render(&device, &queue, &input_view, Some(&input_tex), [4, 4]);
        let start = std::time::Instant::now();
        while output.latest_frame().is_none() && start.elapsed() < std::time::Duration::from_secs(5)
        {
            output.poll_readback(&device);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let pixels = output.latest_frame().expect("readback should complete after resize");
        assert_eq!(pixels.len(), 4 * 4 * 4);
        assert_eq!(&pixels[0..4], &[10, 20, 30, 255]);
    }
}
