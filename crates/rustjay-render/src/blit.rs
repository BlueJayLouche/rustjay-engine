//! Screen blit pipeline: copies the render target to the swap-chain surface.
//! Applies HSB (hue-shift / saturation / brightness) colour correction when
//! any value departs from identity.  When all three are at identity the
//! fragment shader returns early (uniform-flow-control branch — zero extra
//! ALU cost because the whole quad takes the same path).

use rustjay_core::Vertex;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct HsbUniform {
    /// (hue_shift_degrees, saturation_mult, brightness_mult, enabled: 0.0|1.0)
    values: [f32; 4],
}

pub(crate) struct BlitPipeline {
    pipeline: wgpu::RenderPipeline,
    pipeline_bgra8unorm: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
}

impl BlitPipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Blit Shader"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
                struct VertexOutput {
                    @builtin(position) position: vec4<f32>,
                    @location(0) texcoord: vec2<f32>,
                };

                struct HsbUniforms {
                    // hue_shift_degrees, saturation_mult, brightness_mult, enabled (0|1)
                    values: vec4<f32>,
                };

                @group(0) @binding(0) var source_tex:     texture_2d<f32>;
                @group(0) @binding(1) var source_sampler: sampler;
                @group(1) @binding(0) var<uniform> hsb:   HsbUniforms;

                @vertex
                fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> VertexOutput {
                    var out: VertexOutput;
                    out.position = vec4<f32>(position, 0.0, 1.0);
                    out.texcoord = texcoord;
                    return out;
                }

                // Branchless RGB→HSV (Iñigo Quílez)
                fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
                    let K  = vec4<f32>(0.0, -1.0/3.0, 2.0/3.0, -1.0);
                    let p  = mix(vec4<f32>(c.bg, K.wz), vec4<f32>(c.gb, K.xy), step(c.b, c.g));
                    let q  = mix(vec4<f32>(p.xyw, c.r), vec4<f32>(c.r, p.yzx), step(p.x, c.r));
                    let d  = q.x - min(q.w, q.y);
                    let e  = 1.0e-10;
                    return vec3<f32>(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
                }

                // Branchless HSV→RGB
                fn hsv_to_rgb(hsv: vec3<f32>) -> vec3<f32> {
                    let p = abs(fract(vec3<f32>(hsv.x) + vec3<f32>(0.0, 2.0/3.0, 1.0/3.0)) * 6.0 - vec3<f32>(3.0));
                    return hsv.z * mix(vec3<f32>(1.0), clamp(p - vec3<f32>(1.0), vec3<f32>(0.0), vec3<f32>(1.0)), hsv.y);
                }

                @fragment
                fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
                    var color = textureSample(source_tex, source_sampler, in.texcoord);
                    // Uniform-flow-control early return — whole quad takes same branch.
                    if hsb.values.w < 0.5 { return color; }
                    var hsv = rgb_to_hsv(color.rgb);
                    hsv.x = fract(hsv.x + hsb.values.x / 360.0);
                    hsv.y = clamp(hsv.y * hsb.values.y, 0.0, 1.0);
                    hsv.z = clamp(hsv.z * hsb.values.z, 0.0, 1.0);
                    return vec4<f32>(hsv_to_rgb(hsv), color.a);
                }
                "#
                .into(),
            ),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blit Bind Group Layout"),
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
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blit HSB Uniform BGL"),
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

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Initialise with identity (enabled=0 → passthrough).
        let identity = HsbUniform { values: [0.0, 1.0, 1.0, 0.0] };
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label:    Some("Blit HSB Uniform"),
            contents: bytemuck::bytes_of(&identity),
            usage:    wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:   Some("Blit HSB BG"),
            layout:  &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding:  0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Blit Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout), Some(&uniform_bgl)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Blit Pipeline"),
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
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let pipeline_bgra8unorm = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Blit Pipeline (Bgra8Unorm)"),
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
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self { pipeline, pipeline_bgra8unorm, bind_group_layout, sampler, uniform_buffer, uniform_bind_group }
    }

    /// Write HSB values to the uniform buffer.  Call once per frame before `blit()`.
    /// When `enabled` is false (or all values are at identity), the shader takes the
    /// early-return path and performs no colour math.
    pub fn upload_hsb(
        &self,
        queue: &wgpu::Queue,
        hue_shift: f32,
        saturation: f32,
        brightness: f32,
        enabled: bool,
    ) {
        let active = enabled && (hue_shift != 0.0 || saturation != 1.0 || brightness != 1.0);
        let u = HsbUniform {
            values: [hue_shift, saturation, brightness, if active { 1.0 } else { 0.0 }],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&u));
    }

    pub fn create_bind_group(&self, device: &wgpu::Device, source_view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blit Bind Group"),
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
            ],
        })
    }

    pub fn blit(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        bind_group: &wgpu::BindGroup,
        dest_view: &wgpu::TextureView,
        vertex_buffer: &wgpu::Buffer,
        format: wgpu::TextureFormat,
    ) {
        let pipeline = if format == wgpu::TextureFormat::Bgra8Unorm {
            &self.pipeline_bgra8unorm
        } else {
            &self.pipeline
        };
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Blit Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dest_view,
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
        render_pass.set_pipeline(pipeline);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.set_bind_group(1, &self.uniform_bind_group, &[]);
        render_pass.draw(0..6, 0..1);
    }
}
