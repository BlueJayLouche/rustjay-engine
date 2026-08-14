//! Pixel sampler — downsamples canvas regions to tiny grids for DMX pixel
//! mapping. Each segment gets an `cols×rows` Rgba8Unorm target rendered with a
//! linear-filter blit of its canvas region, then an async buffer readback.
//! Reads complete one frame later; a segment still in flight is skipped.
//!
//! Region UVs travel in the vertex buffer, not a uniform struct — no WGSL
//! struct-alignment to get wrong (see the projection black-output postmortem).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use wgpu::util::DeviceExt;
use wgpu::{Device, Queue, TextureFormat, TextureView};

const SHADER: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.pos = vec4<f32>(pos, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src, samp, in.uv);
}
"#;

/// One segment's GPU state.
struct SegState {
    cols: u32,
    rows: u32,
    texture: wgpu::Texture,
    view: TextureView,
    vertices: wgpu::Buffer,
    readback: wgpu::Buffer,
    padded_row: u32,
    /// Set by the map_async callback when the readback is mappable.
    ready: Arc<AtomicBool>,
    in_flight: bool,
    /// Region baked into `vertices` (rewritten when it changes).
    region: [f32; 4],
}

pub struct PixelSampler {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    segs: HashMap<u32, SegState>,
}

/// 6 verts: fullscreen quad (two triangles), UVs = the segment's canvas region.
fn quad_vertices(region: [f32; 4]) -> [[f32; 4]; 6] {
    let [x, y, w, h] = region;
    let (u0, v0, u1, v1) = (x, y, x + w, y + h);
    // pos.xy (NDC), uv.xy — NDC y up, uv y down.
    [
        [-1.0, -1.0, u0, v1],
        [1.0, -1.0, u1, v1],
        [1.0, 1.0, u1, v0],
        [-1.0, -1.0, u0, v1],
        [1.0, 1.0, u1, v0],
        [-1.0, 1.0, u0, v0],
    ]
}

fn padded_bytes_per_row(cols: u32) -> u32 {
    let row = cols * 4;
    row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
}

impl PixelSampler {
    pub fn new(device: &Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pixel-sampler"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pixel-sampler-bgl"),
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
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pixel-sampler-pl"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pixel-sampler-pipeline"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pixel-sampler-linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            sampler,
            segs: HashMap::new(),
        }
    }

    /// Collect finished readbacks: `(segment_id, cols, rows, tightly-packed RGBA)`.
    /// Call once per tick, before [`Self::sample`]. Non-blocking.
    pub fn collect(&mut self, device: &Device) -> Vec<(u32, u32, u32, Vec<u8>)> {
        // Drive the callbacks without waiting.
        let _ = device.poll(wgpu::PollType::Poll);
        let mut out = Vec::new();
        for (id, seg) in self.segs.iter_mut() {
            if !seg.in_flight || !seg.ready.load(Ordering::Acquire) {
                continue;
            }
            let mut data = Vec::with_capacity((seg.cols * seg.rows * 4) as usize);
            {
                let mapped = seg.readback.get_mapped_range(..);
                for row in 0..seg.rows {
                    let start = (row * seg.padded_row) as usize;
                    data.extend_from_slice(&mapped[start..start + (seg.cols * 4) as usize]);
                }
            }
            seg.readback.unmap();
            seg.ready.store(false, Ordering::Release);
            seg.in_flight = false;
            out.push((*id, seg.cols, seg.rows, data));
        }
        out
    }

    /// Kick a downsample+readback pass for each segment not already in flight.
    /// `segments`: `(source view, id, region [x,y,w,h] normalized, cols, rows)`
    /// — each segment names its own source texture.
    pub fn sample(
        &mut self,
        device: &Device,
        queue: &Queue,
        segments: &[(&TextureView, u32, [f32; 4], u32, u32)],
    ) {
        // Drop state for segments that no longer exist.
        self.segs
            .retain(|id, seg| segments.iter().any(|(_, sid, ..)| sid == id) || seg.in_flight);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pixel-sampler-encode"),
        });
        let mut kicked: Vec<u32> = Vec::new();

        for &(src, id, region, cols, rows) in segments {
            let (cols, rows) = (cols.clamp(1, 512), rows.clamp(1, 512));
            // (Re)build state on first sight or grid resize.
            let rebuild = self
                .segs
                .get(&id)
                .is_none_or(|s| s.cols != cols || s.rows != rows);
            if rebuild {
                if self.segs.get(&id).is_some_and(|s| s.in_flight) {
                    continue; // let the old readback land first
                }
                self.segs
                    .insert(id, self.build_seg(device, region, cols, rows));
            }
            let seg = self.segs.get_mut(&id).unwrap();
            if seg.in_flight {
                continue;
            }
            if seg.region != region {
                queue.write_buffer(
                    &seg.vertices,
                    0,
                    bytemuck::cast_slice(&quad_vertices(region)),
                );
                seg.region = region;
            }

            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("pixel-sampler-bg"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(src),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("pixel-sampler-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &seg.view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.set_vertex_buffer(0, seg.vertices.slice(..));
                pass.draw(0..6, 0..1);
            }
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &seg.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &seg.readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(seg.padded_row),
                        rows_per_image: Some(rows),
                    },
                },
                wgpu::Extent3d {
                    width: cols,
                    height: rows,
                    depth_or_array_layers: 1,
                },
            );
            kicked.push(id);
        }

        if kicked.is_empty() {
            return;
        }
        queue.submit(Some(encoder.finish()));
        for id in kicked {
            let seg = self.segs.get_mut(&id).unwrap();
            seg.in_flight = true;
            let ready = Arc::clone(&seg.ready);
            seg.readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |res| {
                    if res.is_ok() {
                        ready.store(true, Ordering::Release);
                    }
                });
        }
    }

    fn build_seg(&self, device: &Device, region: [f32; 4], cols: u32, rows: u32) -> SegState {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pixel-sampler-target"),
            size: wgpu::Extent3d {
                width: cols,
                height: rows,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("pixel-sampler-quad"),
            contents: bytemuck::cast_slice(&quad_vertices(region)),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let padded_row = padded_bytes_per_row(cols);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pixel-sampler-readback"),
            size: (padded_row * rows) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        SegState {
            cols,
            rows,
            texture,
            view,
            vertices,
            readback,
            padded_row,
            ready: Arc::new(AtomicBool::new(false)),
            in_flight: false,
            region,
        }
    }
}
