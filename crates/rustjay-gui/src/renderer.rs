//! # ImGui Renderer
//!
//! wgpu-based renderer for Dear ImGui.

use anyhow::Result;
use std::sync::Arc;
use winit::window::Window;

/// ImGui renderer using wgpu
pub struct ImGuiRenderer {
    context: imgui::Context,
    renderer: imgui_wgpu::Renderer,
    platform: imgui_winit_support::WinitPlatform,
    window: Arc<Window>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    preview_texture_ids: Vec<imgui::TextureId>,
    scale_factor: f64,
}

impl ImGuiRenderer {
    /// Create a new ImGui renderer
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

        // Create ImGui context
        let mut context = imgui::Context::create();
        context.set_ini_filename(None);

        // Set up platform
        let mut platform = imgui_winit_support::WinitPlatform::new(&mut context);
        platform.attach_window(context.io_mut(), &window, imgui_winit_support::HiDpiMode::Rounded);

        // Set display size (in logical points, not physical pixels) and scale
        let logical_width = size.width as f32 / scale_factor as f32;
        let logical_height = size.height as f32 / scale_factor as f32;
        context.io_mut().display_size = [logical_width, logical_height];
        context.io_mut().display_framebuffer_scale = [scale_factor as f32, scale_factor as f32];

        // Style configuration
        let style = context.style_mut();
        style.window_rounding = 4.0;
        style.frame_rounding = 4.0;
        style.scrollbar_rounding = 4.0;

        // Create renderer
        let renderer_config = imgui_wgpu::RendererConfig {
            texture_format: surface_format,
            ..Default::default()
        };

        let renderer = imgui_wgpu::Renderer::new(&mut context, &device, &queue, renderer_config);

        Ok(Self {
            context,
            renderer,
            platform,
            window,
            device,
            queue,
            surface,
            surface_config,
            preview_texture_ids: Vec::new(),
            scale_factor,
        })
    }

    /// Handle window event
    pub fn handle_event(&mut self, event: &winit::event::Event<()>) {
        use winit::event::{Event, MouseScrollDelta, TouchPhase, WindowEvent};
        // imgui-winit-support collapses PixelDelta to ±1.0, discarding magnitude.
        // On macOS the trackpad fires many small PixelDelta events per second, so
        // the sign-only mapping causes jumpy, over-sensitive scrolling. Instead,
        // convert proportionally: 20 logical pixels of trackpad travel = 1 scroll line.
        // LineDelta (discrete mouse wheel clicks) is passed through unchanged.
        if let Event::WindowEvent {
            window_id,
            event: WindowEvent::MouseWheel { delta, phase, .. },
        } = event
        {
            if window_id == &self.window.id() && *phase == TouchPhase::Moved {
                let (h, v) = match delta {
                    MouseScrollDelta::LineDelta(h, v) => (*h, *v),
                    MouseScrollDelta::PixelDelta(pos) => {
                        let pos = pos.to_logical::<f32>(self.scale_factor);
                        (pos.x / 20.0, pos.y / 20.0)
                    }
                };
                self.context.io_mut().add_mouse_wheel_event([h, v]);
                return;
            }
        }
        self.platform.handle_event(self.context.io_mut(), &self.window, event);
    }

    /// Set display size (in logical points)
    pub fn set_display_size(&mut self, width: f32, height: f32) {
        self.context.io_mut().display_size = [width, height];
    }
    
    /// Update scale factor (call when window moves to a different display)
    pub fn set_scale_factor(&mut self, scale_factor: f64) {
        self.scale_factor = scale_factor;
        let sf = scale_factor as f32;
        self.context.io_mut().display_framebuffer_scale = [sf, sf];
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

    /// Create a preview texture for ImGui display
    pub fn create_preview_texture(&mut self, width: u32, height: u32) -> imgui::TextureId {
        let texture_config = imgui_wgpu::TextureConfig {
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            format: Some(wgpu::TextureFormat::Bgra8Unorm),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            ..Default::default()
        };

        let texture = imgui_wgpu::Texture::new(&self.device, &self.renderer, texture_config);
        let texture_id = self.renderer.textures.insert(texture);
        self.preview_texture_ids.push(texture_id);
        texture_id
    }

    /// Update a preview texture with texture data
    pub fn update_preview_texture(
        &mut self,
        texture_id: imgui::TextureId,
        source_texture: &wgpu::Texture,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        // Get the imgui texture
        if let Some(imgui_tex) = self.renderer.textures.get(texture_id) {
            // Copy from source texture to ImGui texture
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: source_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: imgui_tex.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: source_texture.width().min(imgui_tex.width()),
                    height: source_texture.height().min(imgui_tex.height()),
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// Render a frame
    pub fn render_frame<F>(&mut self, build_ui: F) -> Result<()>
    where
        F: FnOnce(&mut imgui::Ui),
    {
        // Prepare frame
        let io = self.context.io_mut();
        self.platform.prepare_frame(io, &self.window)?;

        // Build UI
        let mut ui = self.context.frame();
        build_ui(&mut ui);
        // ui borrows context mutably; NLL ends that borrow here so render()
        // can be called below.  We call render() before acquiring the surface
        // so that if surface acquisition fails the frame is still ended and
        // the next NewFrame() won't assert.
        let draw_data = self.context.render();

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

        // Create encoder
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ImGui Encoder"),
            });

        // Render
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ImGui Render Pass"),
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
                .render(draw_data, &self.queue, &self.device, &mut render_pass)?;
        }

        // Submit
        self.queue.submit(std::iter::once(encoder.finish()));
        surface_texture.present();

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
