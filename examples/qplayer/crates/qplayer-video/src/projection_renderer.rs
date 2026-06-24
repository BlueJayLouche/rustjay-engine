//! Slice + edge-blend renderer for one projector output.

use qplayer_core::{EdgeBlend, EdgeBlendEdge, ProjectorOutput};
use wgpu::util::DeviceExt;
use wgpu::{Device, Queue, RenderPipeline, Sampler, TextureFormat, TextureView};

const VERTICES: &[Vertex] = &[
    Vertex { position: [-1.0, -1.0], texcoord: [0.0, 1.0] },
    Vertex { position: [ 1.0, -1.0], texcoord: [1.0, 1.0] },
    Vertex { position: [-1.0,  1.0], texcoord: [0.0, 0.0] },
    Vertex { position: [ 1.0,  1.0], texcoord: [1.0, 0.0] },
];

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    texcoord: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    source_uv_min: [f32; 2],
    _pad0: [f32; 2],
    source_uv_max: [f32; 2],
    _pad1: [f32; 2],
    output_size: [f32; 2],
    _pad2: [f32; 2],
    edge_left: [f32; 3],
    _pad3: f32,
    edge_right: [f32; 3],
    _pad4: f32,
    edge_top: [f32; 3],
    _pad5: f32,
    edge_bottom: [f32; 3],
    _pad6: f32,
}

impl Uniforms {
    fn new(
        source_rect: [u32; 4],
        canvas_size: [u32; 2],
        output_size: [u32; 2],
        edge_blend: &EdgeBlend,
    ) -> Self {
        let sx = source_rect[0] as f32;
        let sy = source_rect[1] as f32;
        let sw = source_rect[2] as f32;
        let sh = source_rect[3] as f32;
        let cw = canvas_size[0] as f32;
        let ch = canvas_size[1] as f32;

        // Pixel-center aligned UVs for exact 1:1 sampling when sizes match.
        let u_min = (sx + 0.5) / cw;
        let v_min = (sy + 0.5) / ch;
        let u_max = (sx + sw - 0.5) / cw;
        let v_max = (sy + sh - 0.5) / ch;

        Self {
            source_uv_min: [u_min, v_min],
            _pad0: [0.0; 2],
            source_uv_max: [u_max, v_max],
            _pad1: [0.0; 2],
            output_size: [output_size[0] as f32, output_size[1] as f32],
            _pad2: [0.0; 2],
            edge_left: edge_uniform(&edge_blend.left),
            _pad3: 0.0,
            edge_right: edge_uniform(&edge_blend.right),
            _pad4: 0.0,
            edge_top: edge_uniform(&edge_blend.top),
            _pad5: 0.0,
            edge_bottom: edge_uniform(&edge_blend.bottom),
            _pad6: 0.0,
        }
    }
}

fn edge_uniform(edge: &EdgeBlendEdge) -> [f32; 3] {
    [
        if edge.enabled { 1.0 } else { 0.0 },
        edge.width as f32,
        edge.gamma,
    ]
}

/// Renders one projector output: samples a canvas rectangle and applies edge blend.
pub struct ProjectionRenderer {
    pipeline: RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: Sampler,
    uniform_buffer: wgpu::Buffer,
    output_format: TextureFormat,
}

impl ProjectionRenderer {
    pub fn new(device: &Device, output_format: TextureFormat, pixel_perfect: bool) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("projection-bind-group-layout"),
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
            label: Some("projection-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("projection-shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "shaders/projection.wgsl"
            ))),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("projection-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: output_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("projection-vertex-buffer"),
            contents: bytemuck::cast_slice(VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let filter = if pixel_perfect {
            wgpu::FilterMode::Nearest
        } else {
            wgpu::FilterMode::Linear
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("projection-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: filter,
            min_filter: filter,
            ..Default::default()
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("projection-uniform-buffer"),
            size: std::mem::size_of::<Uniforms>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            vertex_buffer,
            bind_group_layout,
            sampler,
            uniform_buffer,
            output_format,
        }
    }

    pub fn output_format(&self) -> TextureFormat {
        self.output_format
    }

    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
    }

    /// Render this output slice into `output_view`.
    pub fn render(
        &self,
        device: &Device,
        queue: &Queue,
        encoder: &mut wgpu::CommandEncoder,
        canvas_view: &TextureView,
        output_view: &TextureView,
        output: &ProjectorOutput,
        canvas_size: [u32; 2],
    ) {
        let uniforms = Uniforms::new(
            [output.source_x, output.source_y, output.source_width, output.source_height],
            canvas_size,
            [output.output_width, output.output_height],
            &output.edge_blend,
        );

        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("projection-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(canvas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        });

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("projection-render-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
        });

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.draw(0..4, 0..1);
    }
}
