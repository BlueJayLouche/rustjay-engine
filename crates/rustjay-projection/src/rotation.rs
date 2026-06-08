//! Output rotation stage — rotates the final image by 0°/90°/180°/270°.
//!
//! Used for physically mounted projectors that are sideways or upside-down.

use crate::identity::BlitVertex;
use crate::stage::ProjectionStage;
use rustjay_core::RenderCtx;
use wgpu::util::DeviceExt;

/// Shared rotation state between the UI (writer) and the stage (reader).
#[derive(Debug, Clone, Default)]
pub struct RotationSync {
    pub rotation: u32,
    pub version: u64,
}

impl RotationSync {
    pub fn set_rotation(&mut self, rotation: u32) {
        if self.rotation != rotation {
            self.rotation = rotation;
            self.version = self.version.wrapping_add(1);
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct RotationParams {
    rotation: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

/// Fullscreen-triangle pipeline with optional UV rotation.
pub struct RotationPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params_buffer: wgpu::Buffer,
    vertex_buffer: wgpu::Buffer,
}

impl RotationPipeline {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rotation Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/rotation.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rotation BGL"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rotation Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Rotation Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[BlitVertex::desc()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Rotation Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rotation Params"),
            contents: bytemuck::cast_slice(&[RotationParams {
                rotation: 0,
                _pad0: 0,
                _pad1: 0,
                _pad2: 0,
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let vertices: &[BlitVertex] = &[
            BlitVertex {
                position: [-1.0, -1.0],
                texcoord: [0.0, 1.0],
            },
            BlitVertex {
                position: [1.0, -1.0],
                texcoord: [1.0, 1.0],
            },
            BlitVertex {
                position: [-1.0, 1.0],
                texcoord: [0.0, 0.0],
            },
            BlitVertex {
                position: [-1.0, 1.0],
                texcoord: [0.0, 0.0],
            },
            BlitVertex {
                position: [1.0, -1.0],
                texcoord: [1.0, 1.0],
            },
            BlitVertex {
                position: [1.0, 1.0],
                texcoord: [1.0, 0.0],
            },
        ];
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rotation Vertex Buffer"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            params_buffer,
            vertex_buffer,
        }
    }

    pub fn set_rotation(&self, queue: &wgpu::Queue, rotation: u32) {
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::cast_slice(&[RotationParams {
                rotation,
                _pad0: 0,
                _pad1: 0,
                _pad2: 0,
            }]),
        );
    }

    pub fn create_bind_group(&self, device: &wgpu::Device, source_view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rotation Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.params_buffer.as_entire_binding(),
                },
            ],
        })
    }
}

/// Projection stage that rotates the input texture by 0°/90°/180°/270°.
pub struct RotationStage {
    pipeline: RotationPipeline,
    sync: std::sync::Arc<std::sync::Mutex<RotationSync>>,
    last_version: u64,
    cached_bind_group: Option<wgpu::BindGroup>,
    cached_input_ptr: Option<usize>,
}

impl RotationStage {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        sync: std::sync::Arc<std::sync::Mutex<RotationSync>>,
    ) -> Self {
        Self {
            pipeline: RotationPipeline::new(device, format),
            sync,
            last_version: 0,
            cached_bind_group: None,
            cached_input_ptr: None,
        }
    }
}

impl ProjectionStage for RotationStage {
    fn label(&self) -> &str {
        "rotation"
    }

    fn render(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        input: &wgpu::TextureView,
        _input_texture: Option<&wgpu::Texture>,
        output: &wgpu::TextureView,
        _output_size: [u32; 2],
    ) {
        let (rotation, version) = {
            let g = self.sync.lock().unwrap_or_else(|e| e.into_inner());
            (g.rotation, g.version)
        };

        if self.last_version != version {
            self.last_version = version;
            self.pipeline.set_rotation(ctx.queue, rotation);
            // Version changed — cached bind group may still be valid (texture unchanged),
            // but force rebind to be safe.
            self.cached_bind_group = None;
        }

        let input_ptr = input as *const _ as usize;
        let bind_group = if self.cached_input_ptr == Some(input_ptr) && self.cached_bind_group.is_some() {
            self.cached_bind_group.as_ref().unwrap()
        } else {
            let bg = self.pipeline.create_bind_group(ctx.device, input);
            self.cached_bind_group = Some(bg);
            self.cached_input_ptr = Some(input_ptr);
            self.cached_bind_group.as_ref().unwrap()
        };

        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Rotation Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
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
        pass.set_pipeline(&self.pipeline.pipeline);
        pass.set_vertex_buffer(0, self.pipeline.vertex_buffer.slice(..));
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..6, 0..1);
    }

    fn on_input_changed(&mut self, _device: &wgpu::Device, _size: [u32; 2]) {
        self.cached_bind_group = None;
        self.cached_input_ptr = None;
    }

    fn is_active(&self) -> bool {
        let g = self.sync.lock().unwrap_or_else(|e| e.into_inner());
        g.rotation != 0
    }
}
