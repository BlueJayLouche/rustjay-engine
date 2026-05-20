//! Waaaves — Multi-pass feedback pipeline (engine port).

use rustjay_engine::prelude::*;

mod params;
mod render;
mod state;
mod tabs;
mod uniforms;

use state::*;
use uniforms::*;

#[allow(dead_code)]
pub struct WaaavesEffect {
    // Pipelines
    pipeline_a: Option<wgpu::RenderPipeline>,
    pipeline_b: Option<wgpu::RenderPipeline>,
    pipeline_c: Option<wgpu::RenderPipeline>,
    // Bind group layouts
    bgl_a: Option<wgpu::BindGroupLayout>,
    bgl_b: Option<wgpu::BindGroupLayout>,
    bgl_c: Option<wgpu::BindGroupLayout>,
    bgl_uniform: Option<wgpu::BindGroupLayout>,
    // Intermediate textures
    intermediate_a: Option<(wgpu::Texture, wgpu::TextureView)>,
    intermediate_b: Option<(wgpu::Texture, wgpu::TextureView)>,
    // Ring buffers
    fb1: Option<render::ring_buffer::RingBuffer>,
    fb2: Option<render::ring_buffer::RingBuffer>,
    // Uniform buffer
    uniform_buf: Option<wgpu::Buffer>,
    uniform_bg: Option<wgpu::BindGroup>,
    // Sampler
    sampler: Option<wgpu::Sampler>,
    // Dummy black texture
    dummy: Option<(wgpu::Texture, wgpu::TextureView)>,
    // Cached resolution for resize detection
    cached_width: u32,
    cached_height: u32,
    // Cached max delay frames
    cached_max_delay: u32,
}

impl Default for WaaavesEffect {
    fn default() -> Self {
        Self {
            pipeline_a: None,
            pipeline_b: None,
            pipeline_c: None,
            bgl_a: None,
            bgl_b: None,
            bgl_c: None,
            bgl_uniform: None,
            intermediate_a: None,
            intermediate_b: None,
            fb1: None,
            fb2: None,
            uniform_buf: None,
            uniform_bg: None,
            sampler: None,
            dummy: None,
            cached_width: 0,
            cached_height: 0,
            cached_max_delay: 0,
        }
    }
}

impl EffectPlugin for WaaavesEffect {
    type State = WaaavesState;
    type Uniforms = WaaavesUniforms;

    fn app_name(&self) -> &str {
        "waaaves"
    }

    fn default_state(&self) -> WaaavesState {
        WaaavesState::default()
    }

    fn shader_source(&self) -> &'static str {
        include_str!("shaders/passthrough.wgsl")
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        params::waaaves_parameter_descriptors()
    }

    fn hidden_tabs(&self) -> Vec<GuiTab> {
        vec![GuiTab::Color, GuiTab::Motion]
    }

    fn init(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        use render::passes;

        self.bgl_a = Some(passes::create_bgl_a(device));
        self.bgl_b = Some(passes::create_bgl_b(device));
        self.bgl_c = Some(passes::create_bgl_c(device));
        self.bgl_uniform = Some(passes::create_bgl_uniform(device));

        let bgl_a = self.bgl_a.as_ref().unwrap();
        let bgl_b = self.bgl_b.as_ref().unwrap();
        let bgl_c = self.bgl_c.as_ref().unwrap();
        let bgl_uniform = self.bgl_uniform.as_ref().unwrap();

        self.pipeline_a = Some(passes::create_pipeline_a(device, bgl_a, bgl_uniform, bgl_b));
        self.pipeline_b = Some(passes::create_pipeline_b(device, bgl_c, bgl_uniform, bgl_b));
        self.pipeline_c = Some(passes::create_pipeline_c(device, bgl_a, bgl_uniform));

        self.sampler = Some(passes::create_sampler(device));
        self.dummy = Some(passes::create_dummy_texture(device, queue));

        let uniform_size = std::mem::size_of::<WaaavesUniforms>() as u64;
        let uniform_buf = passes::create_uniform_buffer(device, uniform_size);
        self.uniform_bg = Some(passes::create_uniform_bind_group(
            device, bgl_uniform, &uniform_buf,
        ));
        self.uniform_buf = Some(uniform_buf);
    }

    fn prepare(
        &mut self,
        state: &mut WaaavesState,
        engine: &EngineState,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        // 1. Uniform upload (every frame)
        let uniforms = WaaavesUniforms::from_state(state, engine);
        queue.write_buffer(
            self.uniform_buf.as_ref().unwrap(),
            0,
            bytemuck::bytes_of(&uniforms),
        );

        // 2. Intermediate texture resize — create or recreate when resolution changes
        let w = engine.resolution.internal_width;
        let h = engine.resolution.internal_height;
        let resolution_changed = w != self.cached_width || h != self.cached_height;

        if resolution_changed || self.intermediate_a.is_none() {
            let create_intermediate = |label: &str| {
                let tex = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
                (tex, view)
            };
            self.intermediate_a = Some(create_intermediate("waaaves_intermediate_a"));
            self.intermediate_b = Some(create_intermediate("waaaves_intermediate_b"));
            self.cached_width = w;
            self.cached_height = h;
        }

        // 3. Ring buffer resize — reallocate when max_delay_frames or resolution changes
        let delay_changed = state.max_delay_frames != self.cached_max_delay;
        if resolution_changed || delay_changed || self.fb1.is_none() {
            let capacity = state.max_delay_frames as usize;
            if let Some(fb1) = self.fb1.as_mut() {
                fb1.resize(device, w, h, capacity);
            } else {
                self.fb1 = Some(render::ring_buffer::RingBuffer::new(device, w, h, capacity));
            }
            if let Some(fb2) = self.fb2.as_mut() {
                fb2.resize(device, w, h, capacity);
            } else {
                self.fb2 = Some(render::ring_buffer::RingBuffer::new(device, w, h, capacity));
            }
            self.cached_max_delay = state.max_delay_frames;
        }
    }

    fn build_uniforms(&self, s: &WaaavesState, engine: &EngineState) -> WaaavesUniforms {
        WaaavesUniforms::from_state(s, engine)
    }

    fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        input_view: Option<&wgpu::TextureView>,
        _input_sampler: Option<&wgpu::Sampler>,
        render_target_view: &wgpu::TextureView,
        state: &mut WaaavesState,
        engine: &EngineState,
        vertex_buffer: &wgpu::Buffer,
        _input_texture: Option<&wgpu::Texture>,
    ) -> bool {
        let dummy_view = &self.dummy.as_ref().unwrap().1;
        let sampler = self.sampler.as_ref().unwrap();
        let uniform_bg = self.uniform_bg.as_ref().unwrap();

        // ch1 = engine primary input; ch2 = engine second input or dummy
        let ch1_view = input_view.unwrap_or(dummy_view);
        let ch2_view = engine.second_input_view.as_deref().unwrap_or(dummy_view);

        let fb1 = self.fb1.as_ref().unwrap();
        let fb2 = self.fb2.as_ref().unwrap();

        let inter_a_view = &self.intermediate_a.as_ref().unwrap().1;
        let inter_b_view = &self.intermediate_b.as_ref().unwrap().1;

        // ── Pass 0 — Block A ────────────────────────────────────────────────
        let fb1_delay = state.block1.fb1_delay_time as usize;
        let fb1_read = fb1.read_view(fb1_delay);

        let bg0_a = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waaaves_a_g0"),
            layout: self.bgl_a.as_ref().unwrap(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(ch1_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(ch2_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        let bg2_a = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waaaves_a_g2"),
            layout: self.bgl_b.as_ref().unwrap(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(fb1_read),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(dummy_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waaaves_pass_a"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: inter_a_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_pipeline(self.pipeline_a.as_ref().unwrap());
            rpass.set_bind_group(0, &bg0_a, &[]);
            rpass.set_bind_group(1, uniform_bg, &[]);
            rpass.set_bind_group(2, &bg2_a, &[]);
            rpass.set_vertex_buffer(0, vertex_buffer.slice(..));
            rpass.draw(0..6, 0..1);
        }

        // ── Pass 1 — Block B ────────────────────────────────────────────────
        let fb2_delay = state.block2.fb2_delay_time as usize;
        let fb2_read = fb2.read_view(fb2_delay);

        let bg0_b = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waaaves_b_g0"),
            layout: self.bgl_c.as_ref().unwrap(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(inter_a_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        let bg2_b = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waaaves_b_g2"),
            layout: self.bgl_b.as_ref().unwrap(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(fb2_read),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(dummy_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waaaves_pass_b"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: inter_b_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_pipeline(self.pipeline_b.as_ref().unwrap());
            rpass.set_bind_group(0, &bg0_b, &[]);
            rpass.set_bind_group(1, uniform_bg, &[]);
            rpass.set_bind_group(2, &bg2_b, &[]);
            rpass.set_vertex_buffer(0, vertex_buffer.slice(..));
            rpass.draw(0..6, 0..1);
        }

        // ── Pass 2 — Block C ────────────────────────────────────────────────
        let bg0_c = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waaaves_c_g0"),
            layout: self.bgl_a.as_ref().unwrap(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(inter_a_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(inter_b_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waaaves_pass_c"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: render_target_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_pipeline(self.pipeline_c.as_ref().unwrap());
            rpass.set_bind_group(0, &bg0_c, &[]);
            rpass.set_bind_group(1, uniform_bg, &[]);
            rpass.set_vertex_buffer(0, vertex_buffer.slice(..));
            rpass.draw(0..6, 0..1);
        }

        // ── Copy outputs to ring buffers and advance ────────────────────────
        let fb1 = self.fb1.as_mut().unwrap();
        let fb2 = self.fb2.as_mut().unwrap();
        let inter_a_tex = &self.intermediate_a.as_ref().unwrap().0;
        let inter_b_tex = &self.intermediate_b.as_ref().unwrap().0;
        let w = engine.resolution.internal_width;
        let h = engine.resolution.internal_height;
        let extent = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };

        encoder.copy_texture_to_texture(
            inter_a_tex.as_image_copy(),
            fb1.write_texture().as_image_copy(),
            extent,
        );
        encoder.copy_texture_to_texture(
            inter_b_tex.as_image_copy(),
            fb2.write_texture().as_image_copy(),
            extent,
        );

        fb1.advance();
        fb2.advance();

        true
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing::span", log::LevelFilter::Warn)
        .init();

    log::info!("Starting RustJay Waaaves v{}", env!("CARGO_PKG_VERSION"));
    rustjay_engine::run_with_tabs(
        WaaavesEffect::default(),
        vec![
            Box::new(tabs::block1_tab::Block1Tab),
            Box::new(tabs::block2_tab::Block2Tab),
            Box::new(tabs::block3_tab::Block3Tab),
        ],
    )
}
