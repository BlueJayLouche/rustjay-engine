//! Waaaves — Multi-pass feedback pipeline (engine port).

use rustjay_engine::prelude::*;

mod legacy_preset;
mod lfo_ui;
mod params;
mod render;
mod state;
mod tabs;
mod uniforms;

use state::*;
use uniforms::*;

#[allow(dead_code)]
#[derive(Default)]
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
    // Per-pass uniform buffers and bind groups
    uniform_buf_a: Option<wgpu::Buffer>,
    uniform_buf_b: Option<wgpu::Buffer>,
    uniform_buf_c: Option<wgpu::Buffer>,
    uniform_bg_a: Option<wgpu::BindGroup>,
    uniform_bg_b: Option<wgpu::BindGroup>,
    uniform_bg_c: Option<wgpu::BindGroup>,
    // Sampler
    sampler: Option<wgpu::Sampler>,
    // Dummy black texture
    dummy: Option<(wgpu::Texture, wgpu::TextureView)>,
    // Cached resolution for resize detection
    cached_width: u32,
    cached_height: u32,
    // Cached max delay frames
    cached_max_delay: u32,
    // Cached bind groups — rebuilt on resize, reused every frame
    fb1_bind_groups: Vec<wgpu::BindGroup>,
    fb2_bind_groups: Vec<wgpu::BindGroup>,
    bg_inter_b: Option<wgpu::BindGroup>,
    bg_inter_c: Option<wgpu::BindGroup>,
}

impl EffectPlugin for WaaavesEffect {
    type State = WaaavesState;
    type Uniforms = WaaavesUniforms;

    fn app_name(&self) -> &str {
        "waaaves"
    }

    /// waaaves blends a second video input (`engine.second_input_view`, see
    /// `render()`), so the engine must keep uploading slot 2.
    fn input_count(&self) -> u32 {
        2
    }

    fn default_state(&self) -> WaaavesState {
        WaaavesState::default()
    }

    fn serialize_preset_state(&self, state: &WaaavesState) -> Option<String> {
        serde_json::to_string(state).ok()
    }

    fn deserialize_preset_state(&self, data: &str, state: &mut WaaavesState) {
        match serde_json::from_str::<WaaavesState>(data) {
            Ok(restored) => *state = restored,
            Err(e) => log::warn!("Failed to deserialize waaaves preset state: {}", e),
        }
    }

    fn on_preset_applied(&self, state: &mut WaaavesState, engine: &mut EngineState) {
        tabs::sync_all_params(state, engine);
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

        let buf_a =
            passes::create_uniform_buffer(device, std::mem::size_of::<BlockAUniforms>() as u64);
        let buf_b =
            passes::create_uniform_buffer(device, std::mem::size_of::<BlockBUniforms>() as u64);
        let buf_c =
            passes::create_uniform_buffer(device, std::mem::size_of::<BlockCUniforms>() as u64);
        self.uniform_bg_a = Some(passes::create_uniform_bind_group(
            device,
            bgl_uniform,
            &buf_a,
        ));
        self.uniform_bg_b = Some(passes::create_uniform_bind_group(
            device,
            bgl_uniform,
            &buf_b,
        ));
        self.uniform_bg_c = Some(passes::create_uniform_bind_group(
            device,
            bgl_uniform,
            &buf_c,
        ));
        self.uniform_buf_a = Some(buf_a);
        self.uniform_buf_b = Some(buf_b);
        self.uniform_buf_c = Some(buf_c);
    }

    fn prepare(
        &mut self,
        state: &mut WaaavesState,
        engine: &EngineState,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        // 1. Uniform upload (every frame) — each pass gets its own buffer at offset 0.
        let uniforms = WaaavesUniforms::from_state(state, engine);
        queue.write_buffer(
            self.uniform_buf_a.as_ref().unwrap(),
            0,
            bytemuck::bytes_of(&uniforms.block_a),
        );
        queue.write_buffer(
            self.uniform_buf_b.as_ref().unwrap(),
            0,
            bytemuck::bytes_of(&uniforms.block_b),
        );
        queue.write_buffer(
            self.uniform_buf_c.as_ref().unwrap(),
            0,
            bytemuck::bytes_of(&uniforms.block_c),
        );

        // Beat-sync delay: recompute fb1/fb2 delay_time from BPM when sync is on
        let bpm = engine.effective_bpm();
        if bpm > 0.0 {
            if state.block1.fb1_delay_time_sync {
                state.block1.fb1_delay_time = sync_to_frames(
                    state.block1.fb1_delay_time_division,
                    bpm,
                    60.0,
                    state.max_delay_frames,
                );
            }
            if state.block2.fb2_delay_time_sync {
                state.block2.fb2_delay_time = sync_to_frames(
                    state.block2.fb2_delay_time_division,
                    bpm,
                    60.0,
                    state.max_delay_frames,
                );
            }
        }

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
                    format: working_format(),
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

            // Rebuild the two bind groups that reference these texture views.
            let sampler = self.sampler.as_ref().unwrap();
            let inter_a_view = &self.intermediate_a.as_ref().unwrap().1;
            let inter_b_view = &self.intermediate_b.as_ref().unwrap().1;
            self.bg_inter_b = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
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
            }));
            self.bg_inter_c = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
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
            }));
        }

        // 3. Ring buffer resize — reallocate when max_delay_frames or resolution changes
        let delay_changed = state.max_delay_frames != self.cached_max_delay;
        if resolution_changed || delay_changed || self.fb1.is_none() {
            let capacity = (state.max_delay_frames as usize).max(1);
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

            // Rebuild per-slot feedback bind groups (one per ring buffer slot).
            // Use a temporary block so all immutable borrows end before the assignment.
            let new_fb1_bgs = {
                let capacity = self.fb1.as_ref().unwrap().capacity();
                let bgl_b = self.bgl_b.as_ref().unwrap();
                let sampler = self.sampler.as_ref().unwrap();
                let dummy_view = &self.dummy.as_ref().unwrap().1;
                let fb1 = self.fb1.as_ref().unwrap();
                (0..capacity)
                    .map(|i| {
                        device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some(&format!("waaaves_fb1_bg_{i}")),
                            layout: bgl_b,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(fb1.slot_view(i)),
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
                        })
                    })
                    .collect::<Vec<_>>()
            };
            let new_fb2_bgs = {
                let capacity = self.fb2.as_ref().unwrap().capacity();
                let bgl_b = self.bgl_b.as_ref().unwrap();
                let sampler = self.sampler.as_ref().unwrap();
                let dummy_view = &self.dummy.as_ref().unwrap().1;
                let fb2 = self.fb2.as_ref().unwrap();
                (0..capacity)
                    .map(|i| {
                        device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some(&format!("waaaves_fb2_bg_{i}")),
                            layout: bgl_b,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(fb2.slot_view(i)),
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
                        })
                    })
                    .collect::<Vec<_>>()
            };
            self.fb1_bind_groups = new_fb1_bgs;
            self.fb2_bind_groups = new_fb2_bgs;
        }
    }

    fn build_uniforms(&self, s: &WaaavesState, engine: &EngineState) -> WaaavesUniforms {
        WaaavesUniforms::from_state(s, engine)
    }

    fn render(&mut self, ctx: &mut RenderHookCtx<'_>, state: &mut WaaavesState) -> bool {
        let dummy_view = &self.dummy.as_ref().unwrap().1;
        let sampler = self.sampler.as_ref().unwrap();
        let uniform_bg_a = self.uniform_bg_a.as_ref().unwrap();
        let uniform_bg_b = self.uniform_bg_b.as_ref().unwrap();
        let uniform_bg_c = self.uniform_bg_c.as_ref().unwrap();

        // ch1 = engine primary input; ch2 = engine second input or dummy
        let ch1_view = ctx.input.map(|i| i.view).unwrap_or(dummy_view);
        let ch2_view = ctx
            .engine_state
            .second_input_view
            .as_deref()
            .unwrap_or(dummy_view);

        // bg0_a is still created per-frame — engine input views may change every frame.
        let bg0_a = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
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

        // Look up all other bind groups from the pre-built caches.
        let fb1_read_idx = self
            .fb1
            .as_ref()
            .unwrap()
            .read_index(state.block1.fb1_delay_time as usize);
        let fb2_read_idx = self
            .fb2
            .as_ref()
            .unwrap()
            .read_index(state.block2.fb2_delay_time as usize);
        let bg2_a = &self.fb1_bind_groups[fb1_read_idx];
        // block2_input_select: 0=Block1(default), 1=Input1, 2=Input2
        let bg0_b_dynamic;
        let bg0_b: &wgpu::BindGroup = match state.block2.block2_input_select {
            1 => {
                bg0_b_dynamic = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("waaaves_b_g0_input1"),
                    layout: self.bgl_c.as_ref().unwrap(),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(ch1_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(sampler),
                        },
                    ],
                });
                &bg0_b_dynamic
            }
            2 => {
                bg0_b_dynamic = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("waaaves_b_g0_input2"),
                    layout: self.bgl_c.as_ref().unwrap(),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(ch2_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(sampler),
                        },
                    ],
                });
                &bg0_b_dynamic
            }
            _ => self.bg_inter_b.as_ref().unwrap(),
        };
        let bg2_b = &self.fb2_bind_groups[fb2_read_idx];
        let bg0_c = self.bg_inter_c.as_ref().unwrap();

        // ── Pass 0 — Block A ────────────────────────────────────────────────
        {
            let mut rpass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waaaves_pass_a"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.intermediate_a.as_ref().unwrap().1,
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
            rpass.set_bind_group(1, uniform_bg_a, &[]);
            rpass.set_bind_group(2, bg2_a, &[]);
            rpass.set_vertex_buffer(0, ctx.vertex_buffer.slice(..));
            rpass.draw(0..6, 0..1);
        }

        // ── Pass 1 — Block B ────────────────────────────────────────────────
        {
            let mut rpass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waaaves_pass_b"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.intermediate_b.as_ref().unwrap().1,
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
            rpass.set_bind_group(0, bg0_b, &[]);
            rpass.set_bind_group(1, uniform_bg_b, &[]);
            rpass.set_bind_group(2, bg2_b, &[]);
            rpass.set_vertex_buffer(0, ctx.vertex_buffer.slice(..));
            rpass.draw(0..6, 0..1);
        }

        // ── Pass 2 — Block C ────────────────────────────────────────────────
        {
            let mut rpass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waaaves_pass_c"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: ctx.target_view,
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
            rpass.set_bind_group(0, bg0_c, &[]);
            rpass.set_bind_group(1, uniform_bg_c, &[]);
            rpass.set_vertex_buffer(0, ctx.vertex_buffer.slice(..));
            rpass.draw(0..6, 0..1);
        }

        // ── Copy outputs to ring buffers and advance ────────────────────────
        let w = ctx.engine_state.resolution.internal_width;
        let h = ctx.engine_state.resolution.internal_height;
        let extent = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        ctx.encoder.copy_texture_to_texture(
            self.intermediate_a.as_ref().unwrap().0.as_image_copy(),
            self.fb1.as_ref().unwrap().write_texture().as_image_copy(),
            extent,
        );
        ctx.encoder.copy_texture_to_texture(
            self.intermediate_b.as_ref().unwrap().0.as_image_copy(),
            self.fb2.as_ref().unwrap().write_texture().as_image_copy(),
            extent,
        );
        self.fb1.as_mut().unwrap().advance();
        self.fb2.as_mut().unwrap().advance();

        true
    }
}

/// Convert a beat-division index to a frame count.
/// Division table: ["1/32","1/16","1/8","1/4","1/2","1","2","4"].
fn sync_to_frames(division_index: i32, bpm: f32, fps: f32, max: u32) -> u32 {
    let mult = match division_index {
        0 => 1.0 / 32.0,
        1 => 1.0 / 16.0,
        2 => 1.0 / 8.0,
        3 => 1.0 / 4.0,
        4 => 1.0 / 2.0,
        5 => 1.0,
        6 => 2.0,
        7 => 4.0,
        _ => 1.0,
    };
    let frames = mult * (60.0 / bpm) * fps;
    frames.round().clamp(1.0, max as f32) as u32
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_to_frames_one_beat_at_120bpm_60fps() {
        // division_index 5 = "1" beat
        assert_eq!(sync_to_frames(5, 120.0, 60.0, 30), 30);
    }

    #[test]
    fn sync_to_frames_clamps_to_max() {
        assert_eq!(sync_to_frames(5, 120.0, 60.0, 10), 10);
    }

    #[test]
    fn sync_to_frames_minimum_one() {
        assert_eq!(sync_to_frames(0, 300.0, 60.0, 30), 1);
    }
}
