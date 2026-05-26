//! `IsfEffect` — loads an ISF GLSL shader at runtime, parses its inputs,
//! transpiles to WGSL, and renders via a custom pipeline.

use std::{collections::HashMap, path::Path, time::Instant};

use isf::{Isf, InputType};
use rustjay_engine::prelude::*;
use wgpu::util::DeviceExt;

use crate::isf_transpiler::{self, MAX_ISF_UNIFORMS};

// ---------------------------------------------------------------------------
// State (serialisable parameter values keyed by ISF input name)
// ---------------------------------------------------------------------------

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct IsfState {
    pub values: HashMap<String, f32>,
}

// ---------------------------------------------------------------------------
// Uniforms: flat [f32; MAX_ISF_UNIFORMS]
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct IsfUniforms([f32; MAX_ISF_UNIFORMS]);

// ---------------------------------------------------------------------------
// IsfEffect
// ---------------------------------------------------------------------------

pub struct IsfEffect {
    pub isf: Isf,
    pub glsl_src: String,
    pub shader_name: String,

    /// Start time — used to compute elapsed seconds for the TIME built-in.
    start_time: Instant,

    /// Error message from transpilation / compilation (shown in GUI).
    pub transpile_error: Option<String>,

    // GPU resources (created in init())
    pipeline:           Option<wgpu::RenderPipeline>,
    texture_bgl:        Option<wgpu::BindGroupLayout>,
    vertex_buffer:      Option<wgpu::Buffer>,
    uniform_buffer:     Option<wgpu::Buffer>,
    uniform_bind_group: Option<wgpu::BindGroup>,

    /// Mapping from ISF input name → slot index in IsfUniforms.
    uniform_index:   Vec<(String, usize)>,
    has_image_input: bool,
}

impl IsfEffect {
    pub fn from_path(path: &Path) -> anyhow::Result<Self> {
        let glsl_src = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Cannot read {}: {}", path.display(), e))?;
        let isf = isf::parse(&glsl_src)
            .map_err(|e| anyhow::anyhow!("ISF parse error in {}: {}", path.display(), e))?;
        let shader_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("ISF Shader")
            .to_string();

        Ok(Self {
            isf,
            glsl_src,
            shader_name,
            start_time: Instant::now(),
            transpile_error: None,
            pipeline: None,
            texture_bgl: None,
            vertex_buffer: None,
            uniform_buffer: None,
            uniform_bind_group: None,
            uniform_index: Vec::new(),
            has_image_input: false,
        })
    }
}

// ---------------------------------------------------------------------------
// EffectPlugin
// ---------------------------------------------------------------------------

impl EffectPlugin for IsfEffect {
    type State    = IsfState;
    type Uniforms = IsfUniforms;

    fn app_name(&self) -> &str { "isf-example" }

    fn shader_source(&self) -> &'static str {
        // The engine compiles this stub, but render() returns true so it is never used.
        include_str!("shaders/passthrough.wgsl")
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        let mut params = Vec::new();
        for input in &self.isf.inputs {
            match &input.ty {
                InputType::Float(f) => {
                    let min = f.min.unwrap_or(0.0);
                    let max = f.max.unwrap_or(1.0);
                    let default = f.default.unwrap_or(0.0);
                    let step = ((max - min) / 100.0).max(0.001);
                    let label = input.label.clone().unwrap_or_else(|| input.name.clone());
                    params.push(ParameterDescriptor::float(
                        &input.name, label,
                        ParamCategory::Custom("ISF".to_string()),
                        min, max, default, step,
                    ));
                }
                InputType::Bool(b) => {
                    let default = b.default.unwrap_or(false);
                    let label = input.label.clone().unwrap_or_else(|| input.name.clone());
                    params.push(ParameterDescriptor::bool(
                        &input.name, label,
                        ParamCategory::Custom("ISF".to_string()),
                        default,
                    ));
                }
                InputType::Long(l) => {
                    let min = l.min.unwrap_or(0);
                    let max = l.max.unwrap_or(10);
                    let default = l.default.unwrap_or(0);
                    let label = input.label.clone().unwrap_or_else(|| input.name.clone());
                    params.push(ParameterDescriptor::int(
                        &input.name, label,
                        ParamCategory::Custom("ISF".to_string()),
                        min, max, default,
                    ));
                }
                _ => {} // image, color, point2D, audio — skipped in Phase 1
            }
        }
        params
    }

    fn default_state(&self) -> IsfState {
        let mut values = HashMap::new();
        for input in &self.isf.inputs {
            match &input.ty {
                InputType::Float(f) => {
                    values.insert(input.name.clone(), f.default.unwrap_or(0.0));
                }
                InputType::Bool(b) => {
                    values.insert(input.name.clone(), if b.default.unwrap_or(false) { 1.0 } else { 0.0 });
                }
                InputType::Long(l) => {
                    values.insert(input.name.clone(), l.default.unwrap_or(0) as f32);
                }
                _ => {}
            }
        }
        IsfState { values }
    }

    fn build_uniforms(&self, state: &IsfState, engine: &EngineState) -> IsfUniforms {
        let mut data = [0.0f32; MAX_ISF_UNIFORMS];

        for (name, idx) in &self.uniform_index {
            if *idx < MAX_ISF_UNIFORMS.saturating_sub(4) {
                let v = engine.get_param(name)
                    .or_else(|| state.values.get(name).copied())
                    .unwrap_or(0.0);
                data[*idx] = v;
            }
        }

        // Built-ins: packed after scalar ISF inputs (16-byte aligned)
        let base = self.uniform_index.len();
        let pad = if base % 4 != 0 { 4 - base % 4 } else { 0 };
        let bi = base + pad;
        if bi + 3 < MAX_ISF_UNIFORMS {
            data[bi]     = engine.resolution.internal_width as f32;
            data[bi + 1] = engine.resolution.internal_height as f32;
            data[bi + 2] = self.start_time.elapsed().as_secs_f32();
            data[bi + 3] = 0.0; // frame index placeholder
        }

        IsfUniforms(data)
    }

    // -----------------------------------------------------------------------
    // Init — transpile + compile ISF pipeline
    // -----------------------------------------------------------------------

    fn init(&mut self, device: &wgpu::Device, _queue: &wgpu::Queue) {
        let transpiled = match isf_transpiler::generate_wgsl(&self.isf, &self.glsl_src) {
            Ok(t) => t,
            Err(e) => {
                self.transpile_error = Some(format!("Transpile error: {}", e));
                log::error!("ISF transpile error: {}", e);
                return;
            }
        };

        self.uniform_index   = transpiled.uniform_index;
        self.has_image_input = transpiled.has_image_input;

        log::debug!("ISF: Generated WGSL for {}:\n{}", self.shader_name, transpiled.wgsl);

        // Compile shader — wgpu panics on WGSL validation errors; catch_unwind prevents crash.
        // Common cause: GLSL function overloading (two fn defs with same name, different types).
        let wgsl = transpiled.wgsl.clone();
        let shader_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ISF Shader"),
                source: wgpu::ShaderSource::Wgsl(wgsl.into()),
            })
        }));
        let shader = match shader_result {
            Ok(s) => s,
            Err(_) => {
                self.transpile_error = Some(
                    "WGSL compilation failed (shader may use unsupported GLSL features like function overloading)"
                        .to_string(),
                );
                log::error!("ISF: WGSL compilation panicked for {}", self.shader_name);
                return;
            }
        };

        // Texture bind group layout (empty when no image input)
        let texture_bgl = if self.has_image_input {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ISF Texture BGL"),
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
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            })
        } else {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ISF No-Texture BGL"),
                entries: &[],
            })
        };

        // Uniform bind group layout
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ISF Uniform BGL"),
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
            label: Some("ISF Pipeline Layout"),
            bind_group_layouts: &[Some(&texture_bgl), Some(&uniform_bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ISF Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Vertex::desc()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
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
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Fullscreen quad
        let vertices = Vertex::quad_vertices();
        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ISF Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // Uniform buffer
        let ub = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ISF Uniform Buffer"),
            size: std::mem::size_of::<IsfUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let ubg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ISF Uniform BG"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: ub.as_entire_binding(),
            }],
        });

        self.pipeline           = Some(pipeline);
        self.texture_bgl        = Some(texture_bgl);
        self.vertex_buffer      = Some(vb);
        self.uniform_buffer     = Some(ub);
        self.uniform_bind_group = Some(ubg);
        self.transpile_error    = None;
    }

    // -----------------------------------------------------------------------
    // Custom render
    // -----------------------------------------------------------------------

    fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        input_view: Option<&wgpu::TextureView>,
        input_sampler: Option<&wgpu::Sampler>,
        render_target_view: &wgpu::TextureView,
        app_state: &mut IsfState,
        engine_state: &EngineState,
        _vertex_buffer: &wgpu::Buffer,
        _input_texture: Option<&wgpu::Texture>,
    ) -> bool {
        let (Some(pipeline), Some(vb), Some(ub), Some(ubg), Some(tex_bgl)) = (
            &self.pipeline,
            &self.vertex_buffer,
            &self.uniform_buffer,
            &self.uniform_bind_group,
            &self.texture_bgl,
        ) else {
            return true; // pipeline not ready — render black
        };

        // Upload uniforms
        let uniforms = self.build_uniforms(app_state, engine_state);
        queue.write_buffer(ub, 0, bytemuck::bytes_of(&uniforms));

        // Build texture bind group
        let texture_bg = if self.has_image_input {
            // If no input connected, skip rendering this frame
            let (Some(iv), Some(s)) = (input_view, input_sampler) else {
                return true;
            };
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ISF Texture BG"),
                layout: tex_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(iv) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(s) },
                ],
            })
        } else {
            // Generator shader — empty bind group
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ISF Empty BG"),
                layout: tex_bgl,
                entries: &[],
            })
        };

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ISF Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: render_target_view,
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

}
