//! # wgpu Renderer
//!
//! Main rendering engine with HSB color manipulation.

use rustjay_core::{HsbParams, EngineState, Vertex};
use rustjay_io::output::OutputManager;
use crate::blit::BlitPipeline;
use crate::pipeline::MainPipeline;
use crate::texture::{InputTexture, Texture};
use crate::uniforms::HsbUniforms;

use anyhow::Result;
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::window::Window;

pub struct WgpuEngine {
    #[allow(dead_code)]
    instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,

    window_width: u32,
    window_height: u32,

    shared_state: Arc<std::sync::Mutex<EngineState>>,

    main_pipeline: MainPipeline,
    blit_pipeline: BlitPipeline,

    pub render_target: Texture,
    pub input_texture: InputTexture,

    vertex_buffer: wgpu::Buffer,
    hsb_uniform_buffer: wgpu::Buffer,

    uniform_bind_group: wgpu::BindGroup,
    blit_bind_group: wgpu::BindGroup,
    cached_texture_bind_group: Option<wgpu::BindGroup>,
    cached_texture_gen: u64,

    frame_count: u64,
    fps_last_time: std::time::Instant,
    fps_frame_count: u32,
    fps_current: f32,

    output_manager: OutputManager,
}

impl WgpuEngine {
    pub async fn new(
        instance: &wgpu::Instance,
        window: Arc<Window>,
        shared_state: Arc<std::sync::Mutex<EngineState>>,
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

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::empty(),
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
            .unwrap_or(surface_caps.formats[0]);

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

        let render_target = Texture::create_render_target(&device, 1920, 1080, "Render Target");
        let input_texture = InputTexture::new(Arc::clone(&device), Arc::clone(&queue));

        let main_pipeline = MainPipeline::new(&device);
        let blit_pipeline = BlitPipeline::new(&device, surface_format);

        let vertices = Vertex::quad_vertices();
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let hsb_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("HSB Uniform Buffer"),
            size: std::mem::size_of::<HsbUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Uniform Bind Group"),
            layout: &main_pipeline.uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: hsb_uniform_buffer.as_entire_binding(),
            }],
        });
        let blit_bind_group = blit_pipeline.create_bind_group(&device, &render_target.view);

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
            main_pipeline,
            blit_pipeline,
            render_target,
            input_texture,
            vertex_buffer,
            hsb_uniform_buffer,
            uniform_bind_group,
            blit_bind_group,
            cached_texture_bind_group: None,
            cached_texture_gen: u64::MAX,
            frame_count: 0,
            fps_last_time: std::time::Instant::now(),
            fps_frame_count: 0,
            fps_current: 0.0,
            output_manager: OutputManager::new(),
        })
    }

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

    pub fn start_ndi_output(&mut self, name: &str, include_alpha: bool) -> anyhow::Result<()> {
        self.output_manager.start_ndi(name, self.render_target.width, self.render_target.height, include_alpha)?;
        Ok(())
    }

    pub fn stop_ndi_output(&mut self) {
        self.output_manager.stop_ndi();
    }

    #[cfg(target_os = "macos")]
    pub fn start_syphon_output(&mut self, server_name: &str) -> anyhow::Result<()> {
        self.output_manager.start_syphon(server_name, Arc::clone(&self.device), Arc::clone(&self.queue))?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub fn stop_syphon_output(&mut self) {
        self.output_manager.stop_syphon();
    }

    #[cfg(target_os = "windows")]
    pub fn start_spout_output(&mut self, sender_name: &str) -> anyhow::Result<()> {
        self.output_manager.start_spout(sender_name)?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    pub fn stop_spout_output(&mut self) {
        self.output_manager.stop_spout();
    }

    #[cfg(target_os = "linux")]
    pub fn start_v4l2_output(&mut self, device_path: &str) -> anyhow::Result<()> {
        self.output_manager.start_v4l2(device_path, self.render_target.width, self.render_target.height)?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub fn stop_v4l2_output(&mut self) {
        self.output_manager.stop_v4l2();
    }

    pub fn render(&mut self, occluded: bool) {
        let (hsb_params, color_enabled) = {
            let state = match self.shared_state.lock() {
                Ok(s) => s,
                Err(e) => e.into_inner(),
            };

            let base_hue = state.audio_routing.base_hue;
            let base_sat = state.audio_routing.base_saturation;
            let base_bright = state.audio_routing.base_brightness;

            let (mut hue, mut sat, mut bright) = if state.audio_routing.enabled {
                state.audio_routing.matrix.apply_to_hsb(base_hue, base_sat, base_bright)
            } else {
                (base_hue, base_sat, base_bright)
            };

            let (hue_mod, sat_mod, bright_mod) = state.lfo.bank.get_hsb_modulations();
            hue = (hue + hue_mod * 90.0).clamp(-180.0, 180.0);
            sat = (sat + sat_mod).clamp(0.0, 2.0);
            bright = (bright + bright_mod).clamp(0.0, 2.0);

            (HsbParams { hue_shift: hue, saturation: sat, brightness: bright }, state.color_enabled)
        };

        if self.input_texture.binding_view().is_none() {
            self.input_texture.ensure_size(1920, 1080);
        }

        let current_gen = self.input_texture.texture_generation;
        if self.cached_texture_gen != current_gen {
            if let (Some(input_view), Some(input_sampler)) = (
                self.input_texture.binding_view(),
                self.input_texture.binding_sampler(),
            ) {
                self.cached_texture_bind_group = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Texture Bind Group"),
                    layout: &self.main_pipeline.texture_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(input_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(input_sampler),
                        },
                    ],
                }));
                self.cached_texture_gen = current_gen;
            }
        }

        let Some(ref texture_bind_group) = self.cached_texture_bind_group else {
            return;
        };

        let hsb_uniforms: HsbUniforms = if color_enabled {
            (&hsb_params).into()
        } else {
            HsbUniforms::identity()
        };
        self.queue.write_buffer(&self.hsb_uniform_buffer, 0, bytemuck::bytes_of(&hsb_uniforms));

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Pipeline Encoder"),
        });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Main Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.render_target.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            render_pass.set_pipeline(&self.main_pipeline.render_pipeline);
            render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            render_pass.set_bind_group(0, texture_bind_group, &[]);
            render_pass.set_bind_group(1, &self.uniform_bind_group, &[]);
            render_pass.draw(0..6, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        self.output_manager.submit_frame(&self.render_target.texture, &self.device, &self.queue);

        if !occluded {
            match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(surface_texture)
                | wgpu::CurrentSurfaceTexture::Suboptimal(surface_texture) => {
                    let surface_view = surface_texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
                    let mut blit_encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Blit Encoder"),
                    });
                    self.blit_pipeline.blit(&mut blit_encoder, &self.blit_bind_group, &surface_view, &self.vertex_buffer);
                    self.queue.submit(std::iter::once(blit_encoder.finish()));
                    surface_texture.present();
                }
                err => {
                    log::debug!("Surface unavailable ({:?}), reconfiguring", err);
                    self.surface.configure(&self.device, &self.surface_config);
                }
            }
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

    pub fn drain_readback(&mut self) {
        self.output_manager.drain_readback(&self.device);
    }
}
