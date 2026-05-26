//! # wgpu Renderer
//!
//! Main rendering engine. Generic over `EffectPlugin` so apps can supply
//! their own shader, uniforms, and GPU resources.

use rustjay_core::{EffectPlugin, EngineState, Vertex};
use rustjay_io::OutputManager;
use crate::blit::BlitPipeline;
use crate::plugin_renderer::PluginRenderer;
use crate::texture::{InputTexture, PreviousFrameTexture, Texture};

use anyhow::Result;
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::window::Window;

/// The main wgpu rendering engine.
pub struct WgpuEngine<P: EffectPlugin> {
    #[allow(dead_code)]
    instance: wgpu::Instance,
    /// GPU adapter used by the engine.
    pub adapter: wgpu::Adapter,
    /// Logical device handle.
    pub device: Arc<wgpu::Device>,
    /// Command queue handle.
    pub queue: Arc<wgpu::Queue>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,

    window_width: u32,
    window_height: u32,

    shared_state: Arc<std::sync::Mutex<EngineState>>,

    plugin_renderer: PluginRenderer<P>,
    blit_pipeline: BlitPipeline,

    /// Main render target texture.
    pub render_target: Texture,
    /// Input texture received from the IO layer (slot 1).
    pub input_texture: InputTexture,
    /// Input texture for slot 2.
    pub second_input_texture: InputTexture,
    /// Cached view for slot 2 (updated when texture generation changes).
    second_input_view: Option<Arc<wgpu::TextureView>>,
    /// Cached sampler for slot 2.
    second_input_sampler: Option<Arc<wgpu::Sampler>>,
    /// Last seen texture generation for slot 2.
    second_input_cached_gen: u64,
    /// Optional feedback texture for previous frame effects.
    pub previous_frame: Option<PreviousFrameTexture>,

    vertex_buffer: wgpu::Buffer,
    blit_bind_group: wgpu::BindGroup,

    frame_count: u64,
    fps_last_time: std::time::Instant,
    fps_frame_count: u32,
    fps_current: f32,
    next_render_time: std::time::Instant,

    output_manager: OutputManager,
}

impl<P: EffectPlugin> WgpuEngine<P> {
    /// Create a new `WgpuEngine` with the given window, state, and plugin.
    pub async fn new(
        instance: &wgpu::Instance,
        window: Arc<Window>,
        shared_state: Arc<std::sync::Mutex<EngineState>>,
        plugin: P,
    ) -> Result<Self> {
        let size = window.inner_size();
        let surface = instance.create_surface(window)?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await?;

        let mut required_features = wgpu::Features::empty();
        #[cfg(not(target_arch = "wasm32"))]
        {
            required_features |= wgpu::Features::POLYGON_MODE_LINE;
        }

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features,
                required_limits: wgpu::Limits::default(),
                label: Some("Device"),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await?;

        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let surface_caps = surface.get_capabilities(&adapter);
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

        let (render_width, render_height) = {
            let s = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (s.resolution.internal_width, s.resolution.internal_height)
        };

        let render_target = Texture::create_render_target(&device, render_width, render_height, "Render Target");
        let input_texture = InputTexture::new(Arc::clone(&device), Arc::clone(&queue));
        let second_input_texture = InputTexture::new(Arc::clone(&device), Arc::clone(&queue));

        let blit_pipeline = BlitPipeline::new(&device, surface_format);

        let vertices = Vertex::quad_vertices();
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let blit_bind_group = blit_pipeline.create_bind_group(&device, &render_target.view);

        let engine_state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
        let plugin_renderer = PluginRenderer::new(plugin, &device, &queue, &engine_state);
        drop(engine_state);

        let previous_frame = if plugin_renderer.cached_graph.as_ref().map(|g| g.feedback).unwrap_or(false) {
            Some(PreviousFrameTexture::new(&device, render_width, render_height))
        } else {
            None
        };

        Ok(Self {
            instance: instance.clone(),
            adapter,
            device: Arc::clone(&device),
            queue: Arc::clone(&queue),
            surface,
            surface_config,
            window_width: size.width,
            window_height: size.height,
            shared_state,
            plugin_renderer,
            blit_pipeline,
            render_target,
            input_texture,
            second_input_texture,
            second_input_view: None,
            second_input_sampler: None,
            second_input_cached_gen: 0,
            previous_frame,
            vertex_buffer,
            blit_bind_group,
            frame_count: 0,
            fps_last_time: std::time::Instant::now(),
            fps_frame_count: 0,
            fps_current: 0.0,
            next_render_time: std::time::Instant::now(),
            output_manager: OutputManager::new(),
        })
    }

    /// Resize the surface to the given dimensions.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.window_width = width;
            self.window_height = height;
            self.surface_config.width = width;
            self.surface_config.height = height;
            self.surface.configure(&self.device, &self.surface_config);
            log::debug!("Resized to {}x{}", width, height);
        }
    }

    /// Recreate the internal render target at a new resolution.
    pub fn resize_render_target(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 { return; }
        if self.render_target.width == width && self.render_target.height == height { return; }
        self.render_target = Texture::create_render_target(&self.device, width, height, "Render Target");
        self.blit_bind_group = self.blit_pipeline.create_bind_group(&self.device, &self.render_target.view);
        if let Some(ref mut pf) = self.previous_frame {
            *pf = PreviousFrameTexture::new(&self.device, width, height);
        }
        self.plugin_renderer.intermediate_textures.clear();
        log::info!("Internal render target resized to {}x{}", width, height);
    }

    /// Start NDI output with the given name.
    pub fn start_ndi_output(&mut self, name: &str, include_alpha: bool) -> anyhow::Result<()> {
        self.output_manager.start_ndi(name, self.render_target.width, self.render_target.height, include_alpha)?;
        Ok(())
    }

    /// Stop NDI output.
    pub fn stop_ndi_output(&mut self) {
        self.output_manager.stop_ndi();
    }

    #[cfg(target_os = "macos")]
    /// Start Syphon output (macOS only).
    pub fn start_syphon_output(&mut self, server_name: &str) -> anyhow::Result<()> {
        self.output_manager.start_syphon(server_name, Arc::clone(&self.device), Arc::clone(&self.queue))?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    /// Stop Syphon output (macOS only).
    pub fn stop_syphon_output(&mut self) {
        self.output_manager.stop_syphon();
    }

    #[cfg(target_os = "windows")]
    /// Start Spout output (Windows only).
    pub fn start_spout_output(&mut self, sender_name: &str) -> anyhow::Result<()> {
        self.output_manager.start_spout(sender_name)?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    /// Stop Spout output (Windows only).
    pub fn stop_spout_output(&mut self) {
        self.output_manager.stop_spout();
    }

    #[cfg(target_os = "linux")]
    /// Start V4L2 output (Linux only).
    pub fn start_v4l2_output(&mut self, device_path: &str) -> anyhow::Result<()> {
        self.output_manager.start_v4l2(device_path, self.render_target.width, self.render_target.height)?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    /// Stop V4L2 output (Linux only).
    pub fn stop_v4l2_output(&mut self) {
        self.output_manager.stop_v4l2();
    }

    /// Render a single frame.
    pub fn render(&mut self, occluded: bool, app_state: &mut P::State) {
        // Frame-rate cap: skip this render if we haven't reached the target interval.
        // Uses a small tolerance to avoid missing frames due to timer jitter on
        // high-refresh displays (e.g. 120 Hz ProMotion) where wake-ups may land
        // a fraction of a millisecond before the exact target time.
        let target_fps = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.target_fps
        };
        let target_interval = std::time::Duration::from_micros(1_000_000 / target_fps.max(1) as u64);
        const CAP_TOLERANCE: std::time::Duration = std::time::Duration::from_micros(1_500); // 1.5 ms
        let now = std::time::Instant::now();
        if now + CAP_TOLERANCE < self.next_render_time {
            return;
        }
        // If we've fallen behind by more than one full interval, reset to avoid
        // a burst of catch-up frames.
        if now > self.next_render_time + target_interval {
            self.next_render_time = now + target_interval;
        } else {
            self.next_render_time += target_interval;
        }

        if self.input_texture.binding_view().is_none() {
            self.input_texture.ensure_size(1920, 1080);
        }

        self.sync_second_input();

        let mut engine_state = match self.shared_state.lock() {
            Ok(s) => s,
            Err(e) => e.into_inner(),
        };
        engine_state.second_input_view = self.second_input_view.clone();
        engine_state.second_input_sampler = self.second_input_sampler.clone();

        // Plugin prepare hook
        self.plugin_renderer.plugin.prepare(
            app_state,
            &engine_state,
            &self.device,
            &self.queue,
        );

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Pipeline Encoder"),
        });

        self.plugin_renderer.render(
            &mut encoder,
            &self.device,
            &self.queue,
            &self.input_texture,
            self.previous_frame.as_ref(),
            &self.render_target,
            app_state,
            &engine_state,
            &self.vertex_buffer,
        );

        // Copy render target to feedback texture for next frame
        if let Some(ref feedback) = self.previous_frame {
            feedback.copy_from(&mut encoder, &self.render_target.texture);
        }

        // GPU pixel readback for color picker
        let pick_uv = engine_state.pick_request.take();
        let readback_buffer: Option<Arc<wgpu::Buffer>> = pick_uv.map(|uv| {
            let x = (uv[0] * self.render_target.width as f32)
                .clamp(0.0, self.render_target.width as f32 - 1.0) as u32;
            let y = (uv[1] * self.render_target.height as f32)
                .clamp(0.0, self.render_target.height as f32 - 1.0) as u32;
            let aligned_row = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer = Arc::new(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Pixel Pick Readback"),
                size: aligned_row as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }));
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.render_target.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(aligned_row),
                        rows_per_image: Some(1),
                    },
                },
                wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            );
            buffer
        });

        // engine_state is no longer needed — drop it now so the FPS tracker
        // below can re-lock shared_state without deadlocking (std::sync::Mutex
        // is not reentrant; holding the guard while calling .lock() again hangs).
        drop(engine_state);

        // Acquire the surface texture now — after all GPU commands are encoded but before
        // submit — so the blit can be appended to the same encoder, saving one Metal
        // command buffer allocation and submission per frame.
        let surface_texture = if !occluded {
            match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(st)
                | wgpu::CurrentSurfaceTexture::Suboptimal(st) => Some(st),
                err => {
                    log::debug!("Surface unavailable ({:?}), reconfiguring", err);
                    self.surface.configure(&self.device, &self.surface_config);
                    None
                }
            }
        } else {
            None
        };

        // Append blit to the main encoder before the single submit.
        if let Some(ref st) = surface_texture {
            let surface_view = st.texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.blit_pipeline.blit(&mut encoder, &self.blit_bind_group, &surface_view, &self.vertex_buffer);
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        if let Some(buffer) = readback_buffer {
            // Time-bounded synchronous readback: a pixel pick is a rare per-click
            // event, but we never block indefinitely in case the GPU/driver hangs.
            use std::sync::atomic::{AtomicBool, Ordering};
            let mapped = std::sync::Arc::new(AtomicBool::new(false));
            let mapped_clone = std::sync::Arc::clone(&mapped);
            buffer.slice(..).map_async(wgpu::MapMode::Read, move |_| {
                mapped_clone.store(true, Ordering::SeqCst);
            });
            let start = std::time::Instant::now();
            let timeout = std::time::Duration::from_secs(5);
            while !mapped.load(Ordering::SeqCst) && start.elapsed() < timeout {
                self.device.poll(wgpu::PollType::Poll).ok();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            if mapped.load(Ordering::SeqCst) {
                let color = {
                    let view = buffer.slice(..).get_mapped_range();
                    let bytes: &[u8] = &view;
                    if bytes.len() >= 4 {
                        // BGRA8: [b, g, r, a]
                        Some([
                            bytes[2] as f32 / 255.0,
                            bytes[1] as f32 / 255.0,
                            bytes[0] as f32 / 255.0,
                        ])
                    } else {
                        None
                    }
                };
                self.shared_state.lock().unwrap_or_else(|e| e.into_inner()).picked_color = color;
            } else {
                log::warn!("Pixel pick readback timed out after 5s");
            }
            buffer.unmap();
        }

        self.output_manager.submit_frame(&self.render_target.texture, &self.device, &self.queue);

        if let Some(st) = surface_texture {
            st.present();
        }

        self.fps_frame_count += 1;
        let elapsed = self.fps_last_time.elapsed();
        if elapsed.as_secs_f32() >= 0.5 {
            self.fps_current = self.fps_frame_count as f32 / elapsed.as_secs_f32();
            self.fps_frame_count = 0;
            self.fps_last_time = std::time::Instant::now();

            if let Ok(mut state) = self.shared_state.lock() {
                state.performance.fps = self.fps_current;
                state.performance.frame_time_ms = if self.fps_current > 0.0 {
                    1000.0 / self.fps_current
                } else {
                    0.0
                };
            }
        }

        self.frame_count += 1;
    }

    /// Update cached view/sampler for the second input texture.
    /// Only recreates GPU handles when the texture generation changes.
    pub fn sync_second_input(&mut self) {
        let current_gen = self.second_input_texture.texture_generation;
        if self.second_input_cached_gen != current_gen {
            self.second_input_cached_gen = current_gen;
            if let Some(ref tex) = self.second_input_texture.texture {
                self.second_input_view = Some(Arc::new(
                    tex.texture.create_view(&wgpu::TextureViewDescriptor::default())
                ));
                self.second_input_sampler = Some(Arc::new(
                    self.device.create_sampler(&wgpu::SamplerDescriptor {
                        address_mode_u: wgpu::AddressMode::ClampToEdge,
                        address_mode_v: wgpu::AddressMode::ClampToEdge,
                        address_mode_w: wgpu::AddressMode::ClampToEdge,
                        mag_filter: wgpu::FilterMode::Linear,
                        min_filter: wgpu::FilterMode::Linear,
                        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                        ..Default::default()
                    })
                ));
            } else {
                self.second_input_view = None;
                self.second_input_sampler = None;
            }
        }
    }

    /// Drain any pending GPU readback operations.
    pub fn drain_readback(&mut self) {
        self.output_manager.drain_readback(&self.device);
    }
}
