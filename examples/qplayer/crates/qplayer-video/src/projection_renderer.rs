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

// Layout must match the WGSL `Uniforms` block. In WGSL `vec2<f32>` aligns to 8
// bytes and `vec3<f32>` to 16, so the two `vec2`s pack together (offsets 0/8/16)
// and each `vec3` edge starts on a 16-byte boundary.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    source_uv_min: [f32; 2], // offset 0
    source_uv_max: [f32; 2], // offset 8
    output_size: [f32; 2],   // offset 16
    _pad0: [f32; 2],         // pad to 32 so edge_left is 16-byte aligned
    edge_left: [f32; 3],     // offset 32
    _pad1: f32,
    edge_right: [f32; 3],    // offset 48
    _pad2: f32,
    edge_top: [f32; 3],      // offset 64
    _pad3: f32,
    edge_bottom: [f32; 3],   // offset 80
    _pad4: f32,
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
            source_uv_max: [u_max, v_max],
            output_size: [output_size[0] as f32, output_size[1] as f32],
            _pad0: [0.0; 2],
            edge_left: edge_uniform(&edge_blend.left),
            _pad1: 0.0,
            edge_right: edge_uniform(&edge_blend.right),
            _pad2: 0.0,
            edge_top: edge_uniform(&edge_blend.top),
            _pad3: 0.0,
            edge_bottom: edge_uniform(&edge_blend.bottom),
            _pad4: 0.0,
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
            bind_group_layouts: &[Some(&bind_group_layout)],
            ..Default::default()
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
            multiview_mask: None,
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
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.draw(0..4, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CanvasTexture, VideoFrame};
    use qplayer_core::CanvasFit;

    fn device_queue() -> (Device, Queue) {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .expect("device");
            (device, queue)
        })
    }

    #[test]
    fn test_projection_renderer_renders_frame() {
        let (device, queue) = device_queue();

        let canvas = CanvasTexture::new(&device, 64, 4);
        let frame = VideoFrame::new(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255,
                0, 0, 255, 255, 255, 255, 0, 255,
            ],
            0.0,
        );
        canvas.upload_frame(&queue, &frame, CanvasFit::Stretch);

        let output_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test-output"),
            size: wgpu::Extent3d {
                width: 64,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let output_view = output_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let renderer = ProjectionRenderer::new(&device, wgpu::TextureFormat::Bgra8UnormSrgb, true);
        let output = ProjectorOutput {
            name: "Output".into(),
            source_x: 0,
            source_y: 0,
            source_width: 64,
            source_height: 4,
            output_width: 64,
            output_height: 4,
            fullscreen_monitor: None,
            edge_blend: EdgeBlend::default(),
        };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("test-encoder"),
        });
        renderer.render(
            &device,
            &queue,
            &mut encoder,
            &canvas.view(),
            &output_view,
            &output,
            [64, 4],
        );
        queue.submit(std::iter::once(encoder.finish()));

        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test-readback"),
            size: 64 * 4 * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("test-copy-encoder"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &output_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(64 * 4),
                    rows_per_image: Some(4),
                },
            },
            wgpu::Extent3d {
                width: 64,
                height: 4,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(encoder.finish()));

        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();

        let data = slice.get_mapped_range();
        assert!(
            data.iter().any(|&b| b != 0),
            "projection renderer produced an all-black output"
        );
    }

    /// Mirrors the real runtime path: 1920x1080 canvas, `Fit`, the default
    /// single output, full-res render target — and checks the *center* pixel,
    /// catching black output that the tiny stretch test above can miss.
    #[test]
    fn test_projection_default_single_fit_center_nonblack() {
        let (device, queue) = device_queue();

        let (cw, ch) = (1920u32, 1080u32);
        let canvas = CanvasTexture::new(&device, cw, ch);
        // Red frame, but with a BLACK top-left quadrant. A uniform-color frame
        // would hide a uniform-buffer layout bug that collapses every output
        // pixel onto the canvas's top-left texel; this asymmetry catches it.
        let mut data = vec![255u8, 0, 0, 255].repeat((cw * ch) as usize);
        for y in 0..ch / 2 {
            for x in 0..cw / 2 {
                let i = ((y * cw + x) * 4) as usize;
                data[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
        let frame = VideoFrame::new(cw, ch, data, 0.0);
        canvas.upload_frame(&queue, &frame, CanvasFit::Fit);

        let output_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test-output-fullres"),
            size: wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let output_view = output_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let renderer = ProjectionRenderer::new(&device, wgpu::TextureFormat::Bgra8UnormSrgb, true);
        let output = ProjectorOutput::default_single();

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("test-encoder-fullres"),
        });
        renderer.render(&device, &queue, &mut encoder, &canvas.view(), &output_view, &output, [cw, ch]);
        queue.submit(std::iter::once(encoder.finish()));

        let bytes_per_row = cw * 4; // 7680, already 256-aligned
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test-readback-fullres"),
            size: (bytes_per_row * ch) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("test-copy-encoder-fullres"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &output_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(ch),
                },
            },
            wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 },
        );
        queue.submit(std::iter::once(encoder.finish()));

        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();

        let data = slice.get_mapped_range();
        // Center pixel (BGRA): red frame -> B low, R high.
        let cx = cw / 2;
        let cy = ch / 2;
        let idx = (cy * bytes_per_row + cx * 4) as usize;
        let (b, g, r, a) = (data[idx], data[idx + 1], data[idx + 2], data[idx + 3]);
        assert!(r > 100, "center pixel not red: bgra=({b},{g},{r},{a})");
    }
}
