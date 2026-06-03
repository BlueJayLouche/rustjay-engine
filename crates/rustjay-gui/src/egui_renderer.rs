//! # Egui Renderer
//!
//! wgpu-based renderer for egui — mirrors `ImGuiRenderer` API.

use anyhow::Result;
use std::sync::Arc;
use winit::window::Window;

/// Egui renderer using wgpu
pub struct EguiRenderer {
    context: egui::Context,
    renderer: egui_wgpu::Renderer,
    state: egui_winit::State,
    window: Arc<Window>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    preview_texture_ids: Vec<egui::TextureId>,
    preview_textures: std::collections::HashMap<egui::TextureId, wgpu::Texture>,
    scale_factor: f64,
}

impl EguiRenderer {
    /// Create a new egui renderer
    pub async fn new(
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        window: Arc<Window>,
        scale_factor: f64,
    ) -> Result<Self> {
        let size = window.inner_size();

        // Create surface
        let surface = instance.create_surface(window.clone())?;

        let surface_caps = surface.get_capabilities(adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| *f == wgpu::TextureFormat::Bgra8UnormSrgb || *f == wgpu::TextureFormat::Bgra8Unorm)
            .or_else(|| surface_caps.formats.first().copied())
            .ok_or_else(|| anyhow::anyhow!("No surface formats available"))?;

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        // Create egui context
        let context = egui::Context::default();

        // Set up winit state
        let native_pixels_per_point = Some(scale_factor as f32);
        let max_texture_side = device.limits().max_texture_dimension_2d as usize;
        let state = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            &window,
            native_pixels_per_point,
            None,
            Some(max_texture_side),
        );

        // Create renderer
        let renderer = egui_wgpu::Renderer::new(
            &device,
            surface_format,
            egui_wgpu::RendererOptions {
                dithering: false,
                ..Default::default()
            },
        );

        Ok(Self {
            context,
            renderer,
            state,
            window,
            device,
            queue,
            surface,
            surface_config,
            preview_texture_ids: Vec::new(),
            preview_textures: std::collections::HashMap::new(),
            scale_factor,
        })
    }

    /// Handle window event
    pub fn handle_event(&mut self, event: &winit::event::Event<()>) {
        if let winit::event::Event::WindowEvent { window_id, event } = event {
            if *window_id == self.window.id() {
                let _ = self.state.on_window_event(&self.window, event);
            }
        }
    }

    /// Set display size (in logical points)
    pub fn set_display_size(&mut self, width: f32, height: f32) {
        // egui-winit handles this automatically via take_egui_input
        let _ = (width, height);
    }

    /// Update scale factor (call when window moves to a different display)
    pub fn set_scale_factor(&mut self, scale_factor: f64) {
        self.scale_factor = scale_factor;
    }

    /// Get current scale factor
    pub fn scale_factor(&self) -> f64 {
        self.scale_factor
    }

    /// Resize surface
    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.surface_config.width = width;
            self.surface_config.height = height;
            self.surface.configure(&self.device, &self.surface_config);
        }
    }

    /// Create a preview texture for egui display
    pub fn create_preview_texture(&mut self, width: u32, height: u32) -> egui::TextureId {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("egui preview texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let texture_id = self.renderer.register_native_texture(
            &self.device,
            &view,
            wgpu::FilterMode::Linear,
        );
        self.preview_texture_ids.push(texture_id);
        self.preview_textures.insert(texture_id, texture);
        texture_id
    }

    /// Get the underlying wgpu texture for a preview.
    pub fn get_preview_texture(&self, texture_id: egui::TextureId) -> Option<&wgpu::Texture> {
        self.preview_textures.get(&texture_id)
    }

    /// Update a preview texture with texture data
    pub fn update_preview_texture(
        &self,
        texture_id: egui::TextureId,
        source_texture: &wgpu::Texture,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        if let Some(dest_texture) = self.preview_textures.get(&texture_id) {
            let width = source_texture.width().min(dest_texture.width());
            let height = source_texture.height().min(dest_texture.height());
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: source_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: dest_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// Render a frame
    pub fn render_frame<F>(&mut self, mut build_ui: F) -> Result<()>
    where
        F: FnMut(&egui::Context),
    {
        // Prepare frame input
        let raw_input = self.state.take_egui_input(&self.window);

        // Run egui
        let full_output = self.context.run_ui(raw_input, |ctx| {
            build_ui(ctx);
        });

        // Handle platform output (cursor, clipboard, etc.)
        self.state
            .handle_platform_output(&self.window, full_output.platform_output);

        // Tessellate shapes into paint jobs
        let paint_jobs = self
            .context
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        // Update textures
        for (id, image_delta) in &full_output.textures_delta.set {
            self.renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }

        // Get surface texture
        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
        };

        let surface_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point: self.scale_factor as f32,
        };

        // Create encoder
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("egui Encoder"),
            });

        // Update buffers (must be called before render)
        let extra_cmd_bufs = self.renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        // Render
        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.1,
                            g: 0.1,
                            b: 0.1,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            self.renderer
                .render(&mut render_pass.forget_lifetime(), &paint_jobs, &screen_descriptor);
        }

        // Submit encoder + any extra command buffers from update_buffers
        let mut submissions: Vec<wgpu::CommandBuffer> = extra_cmd_bufs;
        submissions.push(encoder.finish());
        self.queue.submit(submissions);
        surface_texture.present();

        // Free textures
        for id in &full_output.textures_delta.free {
            self.renderer.free_texture(id);
        }

        Ok(())
    }

    /// Get device reference
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Get queue reference
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
}
