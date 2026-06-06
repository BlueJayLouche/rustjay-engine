//! Delta — RGB delay / motion extraction (egui edition).
//!
//! Uses rustjay-engine with the egui backend and a custom Motion tab.

use rustjay_engine::prelude::*;

// ---------------------------------------------------------------------------
// Blend modes (must match shader switch cases)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[repr(i32)]
enum BlendMode {
    #[default]
    Replace = 0,
    Add = 1,
    Multiply = 2,
    Screen = 3,
    Difference = 4,
    Overlay = 5,
    Lighten = 6,
    Darken = 7,
}

impl BlendMode {
    fn name(&self) -> &'static str {
        match self {
            BlendMode::Replace => "Replace",
            BlendMode::Add => "Add",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Difference => "Difference",
            BlendMode::Overlay => "Overlay",
            BlendMode::Lighten => "Lighten",
            BlendMode::Darken => "Darken",
        }
    }

    const ALL: &'static [BlendMode] = &[
        BlendMode::Replace,
        BlendMode::Add,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Difference,
        BlendMode::Overlay,
        BlendMode::Lighten,
        BlendMode::Darken,
    ];
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
struct DeltaState {
    red_delay: u32,
    green_delay: u32,
    blue_delay: u32,
    intensity: f32,
    blend_mode: BlendMode,
    grayscale_input: bool,
    red_gain: f32,
    green_gain: f32,
    blue_gain: f32,
    input_mix: f32,
    trail_fade: f32,
    threshold: f32,
    smoothing: f32,
    enabled: bool,
}

impl Default for DeltaState {
    fn default() -> Self {
        Self {
            red_delay: 0,
            green_delay: 2,
            blue_delay: 4,
            intensity: 1.0,
            blend_mode: BlendMode::Replace,
            grayscale_input: true,
            red_gain: 1.0,
            green_gain: 1.0,
            blue_gain: 1.0,
            input_mix: 0.0,
            trail_fade: 0.0,
            threshold: 0.0,
            smoothing: 0.0,
            enabled: true,
        }
    }
}

// ---------------------------------------------------------------------------
// GPU uniform block — must match MotionParams in the shader
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DeltaUniforms {
    delays: [f32; 4],
    settings: [f32; 4],
    channel_gain: [f32; 4],
    mix_options: [f32; 4],
}

// ---------------------------------------------------------------------------
// Frame-history ring buffer (GPU→GPU copies)
// ---------------------------------------------------------------------------

struct FrameHistory {
    frames: Vec<Texture>,
    write_index: usize,
    max_history: usize,
    width: u32,
    height: u32,
}

impl FrameHistory {
    const MAX_HISTORY: usize = 16;
    const DEFAULT_HISTORY: usize = 8;

    fn new(device: &wgpu::Device, max_history: usize) -> Self {
        let max_history = max_history.clamp(1, Self::MAX_HISTORY);
        let mut frames = Vec::with_capacity(max_history);
        for i in 0..max_history {
            frames.push(Texture::create_render_target(
                device,
                1920,
                1080,
                &format!("Frame History {}", i),
            ));
        }
        Self {
            frames,
            write_index: 0,
            max_history,
            width: 1920,
            height: 1080,
        }
    }

    fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.width != width || self.height != height {
            self.frames.clear();
            for i in 0..self.max_history {
                self.frames.push(Texture::create_render_target(
                    device,
                    width,
                    height,
                    &format!("Frame History {}", i),
                ));
            }
            self.width = width;
            self.height = height;
        }
    }

    fn push_frame(&mut self, source: &wgpu::Texture, encoder: &mut wgpu::CommandEncoder) {
        let width = source.width();
        let height = source.height();
        let dest = &self.frames[self.write_index];
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: source,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &dest.texture,
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
        self.write_index = (self.write_index + 1) % self.max_history;
    }

    fn get_frame(&self, frames_ago: usize) -> Option<&Texture> {
        if frames_ago >= self.max_history {
            return None;
        }
        let index = if frames_ago < self.write_index {
            self.write_index - 1 - frames_ago
        } else {
            self.max_history - 1 - (frames_ago - self.write_index)
        };
        self.frames.get(index)
    }
}

// ---------------------------------------------------------------------------
// Effect plugin
// ---------------------------------------------------------------------------

#[derive(Default)]
struct DeltaEffect {
    pipeline: Option<wgpu::RenderPipeline>,
    texture_bind_group_layout: Option<wgpu::BindGroupLayout>,
    uniform_bind_group_layout: Option<wgpu::BindGroupLayout>,
    history: Option<FrameHistory>,
    vertex_buffer: Option<wgpu::Buffer>,
    uniform_buffer: Option<wgpu::Buffer>,
    uniform_bind_group: Option<wgpu::BindGroup>,
}

const DELTA_SHADER: &str = include_str!("shaders/delta.wgsl");

impl EffectPlugin for DeltaEffect {
    type State = DeltaState;
    type Uniforms = DeltaUniforms;

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor::int("red_delay", "Red Delay", ParamCategory::Motion, 0, 16, 0),
            ParameterDescriptor::int(
                "green_delay",
                "Green Delay",
                ParamCategory::Motion,
                0,
                16,
                2,
            ),
            ParameterDescriptor::int("blue_delay", "Blue Delay", ParamCategory::Motion, 0, 16, 4),
            ParameterDescriptor::float(
                "intensity",
                "Intensity",
                ParamCategory::Motion,
                0.0,
                1.0,
                1.0,
                0.01,
            ),
            ParameterDescriptor::enum_param(
                "blend_mode",
                "Blend Mode",
                ParamCategory::Motion,
                vec![
                    "Replace".into(),
                    "Add".into(),
                    "Multiply".into(),
                    "Screen".into(),
                    "Difference".into(),
                    "Overlay".into(),
                    "Lighten".into(),
                    "Darken".into(),
                ],
                0,
            ),
            ParameterDescriptor::bool(
                "grayscale_input",
                "Grayscale Input",
                ParamCategory::Motion,
                true,
            ),
            ParameterDescriptor::float(
                "red_gain",
                "Red Gain",
                ParamCategory::Motion,
                -2.0,
                2.0,
                1.0,
                0.01,
            ),
            ParameterDescriptor::float(
                "green_gain",
                "Green Gain",
                ParamCategory::Motion,
                -2.0,
                2.0,
                1.0,
                0.01,
            ),
            ParameterDescriptor::float(
                "blue_gain",
                "Blue Gain",
                ParamCategory::Motion,
                -2.0,
                2.0,
                1.0,
                0.01,
            ),
            ParameterDescriptor::float(
                "input_mix",
                "Input Mix",
                ParamCategory::Motion,
                0.0,
                1.0,
                0.0,
                0.01,
            ),
            ParameterDescriptor::float(
                "trail_fade",
                "Trail Fade",
                ParamCategory::Motion,
                0.0,
                1.0,
                0.0,
                0.01,
            ),
            ParameterDescriptor::float(
                "threshold",
                "Threshold",
                ParamCategory::Motion,
                0.0,
                1.0,
                0.0,
                0.01,
            ),
            ParameterDescriptor::float(
                "smoothing",
                "Smoothing",
                ParamCategory::Motion,
                0.0,
                1.0,
                0.0,
                0.01,
            ),
        ]
    }

    fn hidden_tabs(&self) -> Vec<GuiTab> {
        vec![GuiTab::Color]
    }

    fn shader_source(&self) -> &'static str {
        DELTA_SHADER
    }

    fn default_state(&self) -> DeltaState {
        DeltaState::default()
    }

    fn init(&mut self, device: &wgpu::Device, _queue: &wgpu::Queue) {
        let real_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Delta Motion Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/delta_real.wgsl").into()),
        });

        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Delta Texture BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Delta Uniform BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Delta Pipeline Layout"),
            bind_group_layouts: &[Some(&texture_bgl), Some(&uniform_bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Delta Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &real_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Vertex::desc()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &real_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let vertices = Vertex::quad_vertices();
        use wgpu::util::DeviceExt;
        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Delta Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let ub = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Delta Uniform Buffer"),
            size: std::mem::size_of::<DeltaUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let ubg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Delta Uniform BG"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: ub.as_entire_binding(),
            }],
        });

        self.pipeline = Some(pipeline);
        self.texture_bind_group_layout = Some(texture_bgl);
        self.uniform_bind_group_layout = Some(uniform_bgl);
        self.history = Some(FrameHistory::new(device, FrameHistory::DEFAULT_HISTORY));
        self.vertex_buffer = Some(vb);
        self.uniform_buffer = Some(ub);
        self.uniform_bind_group = Some(ubg);
    }

    fn render(&mut self, ctx: &mut RenderHookCtx<'_>, app_state: &mut Self::State) -> bool {
        let Some(pipeline) = &self.pipeline else {
            return false;
        };
        let Some(vb) = &self.vertex_buffer else {
            return false;
        };
        let Some(ub) = &self.uniform_buffer else {
            return false;
        };
        let Some(ubg) = &self.uniform_bind_group else {
            return false;
        };
        let Some(tex_bgl) = &self.texture_bind_group_layout else {
            return false;
        };

        let uniforms = self.build_uniforms(app_state, ctx.engine_state);

        let Some(history) = &mut self.history else {
            return false;
        };

        if let Some(src) = ctx.input.and_then(|i| i.texture) {
            history.resize(ctx.device, src.width(), src.height());
            history.push_frame(src, ctx.encoder);
        }
        ctx.queue.write_buffer(ub, 0, bytemuck::bytes_of(&uniforms));

        let rd = uniforms.delays[0] as usize;
        let gd = uniforms.delays[1] as usize;
        let bd = uniforms.delays[2] as usize;

        let red_frame = history.get_frame(rd).or_else(|| history.get_frame(0));
        let green_frame = history.get_frame(gd).or_else(|| history.get_frame(0));
        let blue_frame = history.get_frame(bd).or_else(|| history.get_frame(0));

        let default_view = history.get_frame(0).map(|t| &t.view);
        let default_sampler = history.get_frame(0).map(|t| &t.sampler);

        let rv = red_frame.map(|t| &t.view).or(default_view);
        let gv = green_frame.map(|t| &t.view).or(default_view);
        let bv = blue_frame.map(|t| &t.view).or(default_view);
        let iv = ctx.input.map(|i| i.view).or(default_view);
        let s = ctx.input.map(|i| i.sampler).or(default_sampler);

        let (Some(rv), Some(gv), Some(bv), Some(iv), Some(s)) = (rv, gv, bv, iv, s) else {
            return true;
        };

        let texture_bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Delta Texture BG"),
            layout: tex_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(rv),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(gv),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(bv),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(iv),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(s),
                },
            ],
        });

        {
            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Delta Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: ctx.target_view,
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
            pass.set_pipeline(pipeline);
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.set_bind_group(0, &texture_bg, &[]);
            pass.set_bind_group(1, ubg, &[]);
            pass.draw(0..6, 0..1);
        }

        true
    }

    fn build_uniforms(&self, s: &DeltaState, engine: &EngineState) -> DeltaUniforms {
        if !s.enabled {
            return DeltaUniforms {
                delays: [0.0, 0.0, 0.0, 16.0],
                settings: [0.0, 0.0, 0.0, 0.0],
                channel_gain: [1.0, 1.0, 1.0, 0.0],
                mix_options: [1.0, 0.0, 0.0, 0.0],
            };
        }

        let rd = engine
            .get_param("red_delay")
            .unwrap_or(s.red_delay as f32)
            .round();
        let gd = engine
            .get_param("green_delay")
            .unwrap_or(s.green_delay as f32)
            .round();
        let bd = engine
            .get_param("blue_delay")
            .unwrap_or(s.blue_delay as f32)
            .round();
        let intensity = engine.get_param("intensity").unwrap_or(s.intensity);
        let blend_mode = engine
            .get_param("blend_mode")
            .unwrap_or(s.blend_mode as i32 as f32);
        let grayscale = engine
            .get_param("grayscale_input")
            .unwrap_or(if s.grayscale_input { 1.0 } else { 0.0 });
        let red_gain = engine.get_param("red_gain").unwrap_or(s.red_gain);
        let green_gain = engine.get_param("green_gain").unwrap_or(s.green_gain);
        let blue_gain = engine.get_param("blue_gain").unwrap_or(s.blue_gain);
        let input_mix = engine.get_param("input_mix").unwrap_or(s.input_mix);
        let trail_fade = engine.get_param("trail_fade").unwrap_or(s.trail_fade);
        let threshold = engine.get_param("threshold").unwrap_or(s.threshold);
        let smoothing = engine.get_param("smoothing").unwrap_or(s.smoothing);

        DeltaUniforms {
            delays: [rd, gd, bd, 16.0],
            settings: [intensity, blend_mode, grayscale, 0.0],
            channel_gain: [red_gain, green_gain, blue_gain, 0.0],
            mix_options: [input_mix, trail_fade, threshold, smoothing],
        }
    }
}

// ---------------------------------------------------------------------------
// Custom Motion tab (egui)
// ---------------------------------------------------------------------------

struct MotionTab;

impl AnyEguiTab for MotionTab {
    fn name(&self) -> &str {
        "Motion"
    }
    fn replaces(&self) -> Option<GuiTab> {
        Some(GuiTab::Motion)
    }

    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        app_state: &mut dyn std::any::Any,
        engine: &mut EngineState,
    ) {
        let state = app_state
            .downcast_mut::<DeltaState>()
            .expect("MotionTab expects DeltaState");

        let _w = ui.push_id("motion_tab", |ui| {
            ui.heading("RGB Delay / Motion Extraction");
            ui.separator();

            // Enable toggle is not an engine param — stays in local state.
            let mut enabled = state.enabled;
            if ui.checkbox(&mut enabled, "Enabled").changed() {
                state.enabled = enabled;
            }
            ui.separator();

            ui.label(egui::RichText::new("Channel Delays (frames)").strong());
            param_slider_int(ui, engine, "red_delay", "Red", 0, 16);
            param_slider_int(ui, engine, "green_delay", "Green", 0, 16);
            param_slider_int(ui, engine, "blue_delay", "Blue", 0, 16);

            ui.separator();

            param_slider(ui, engine, "intensity", "Intensity", 0.0, 1.0);

            // Blend mode: enum, not a plain float — handled inline.
            let blend_names: Vec<&str> = BlendMode::ALL.iter().map(|b| b.name()).collect();
            let mut blend_idx = engine.get_param_base("blend_mode").unwrap_or(0.0).round() as usize;
            let prev_blend = blend_idx;
            ui.horizontal(|ui| {
                ui.label("Blend Mode:");
                egui::ComboBox::from_id_salt("blend_mode")
                    .width(ui.available_width())
                    .selected_text(blend_names[blend_idx.min(blend_names.len() - 1)])
                    .show_ui(ui, |ui| {
                        for (i, name) in blend_names.iter().enumerate() {
                            if ui.selectable_label(blend_idx == i, *name).clicked() {
                                blend_idx = i;
                            }
                        }
                    });
            });
            if blend_idx != prev_blend {
                engine.set_param_base("blend_mode", blend_idx as f32);
            }

            // Grayscale: bool param — handled inline.
            let mut grayscale = engine.get_param_base("grayscale_input").unwrap_or(1.0) > 0.5;
            if ui.checkbox(&mut grayscale, "Grayscale Input").changed() {
                engine.set_param_base("grayscale_input", if grayscale { 1.0 } else { 0.0 });
            }

            ui.separator();

            ui.label(egui::RichText::new("Channel Gains").strong());
            param_slider(ui, engine, "red_gain", "Red", -2.0, 2.0);
            param_slider(ui, engine, "green_gain", "Green", -2.0, 2.0);
            param_slider(ui, engine, "blue_gain", "Blue", -2.0, 2.0);

            ui.separator();

            ui.label(egui::RichText::new("Mix & Post").strong());
            param_slider(ui, engine, "input_mix", "Input Mix", 0.0, 1.0);
            param_slider(ui, engine, "trail_fade", "Trail Fade", 0.0, 1.0);
            param_slider(ui, engine, "threshold", "Threshold", 0.0, 1.0);
            param_slider(ui, engine, "smoothing", "Smoothing", 0.0, 1.0);
        });
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing::span", log::LevelFilter::Warn)
        .init();

    log::info!(
        "Starting RustJay Delta (egui edition) v{}",
        env!("CARGO_PKG_VERSION")
    );

    rustjay_engine::run_with_egui_tabs(DeltaEffect::default(), vec![Box::new(MotionTab)])
}
