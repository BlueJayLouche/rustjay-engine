//! GPU YUV420 → RGB conversion. Uploads the three decoded planes as R8 textures
//! and runs one fullscreen pass that does the colorspace matrix + canvas fit,
//! writing the projection canvas. Replaces the CPU swscale + the 23 MB/frame
//! RGBA copy with ~8.5 MB of plane uploads (1.5 B/px) and a GPU pass.

use crate::frame::{FramePixels, VideoFrame};
use qplayer_core::CanvasFit;
use wgpu::{Device, Queue, TextureFormat, TextureView};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    dst_min: [f32; 2],
    dst_max: [f32; 2],
    src_min: [f32; 2],
    src_max: [f32; 2],
    full_range: f32,
    bt709: f32,
    _pad: [f32; 2],
}

struct Planes {
    y: wgpu::Texture,
    u: wgpu::Texture,
    v: wgpu::Texture,
    y_dim: (u32, u32),
    c_dim: (u32, u32),
    bind_group: wgpu::BindGroup,
}

pub struct YuvConverter {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    planes: Option<Planes>,
}

impl YuvConverter {
    pub fn new(device: &Device, target_format: TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv-convert-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuv-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                plane_entry(1),
                plane_entry(2),
                plane_entry(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
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

        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("yuv-pl"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("yuv-convert"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
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

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("yuv-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("yuv-uniform"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self { pipeline, layout, sampler, uniform, planes: None }
    }

    /// Upload `frame`'s three planes + the fit/colourspace uniform. Pairs with
    /// `encode`, which records the conversion pass (split so the pass can be folded
    /// into an output's command encoder, avoiding a separate submit).
    pub fn upload(
        &mut self,
        device: &Device,
        queue: &Queue,
        frame: &VideoFrame,
        canvas_size: [u32; 2],
        fit: CanvasFit,
    ) {
        let FramePixels::Yuv420 { y, u, v, full_range, bt709 } = &frame.pixels else {
            return; // caller guarantees YUV; nothing to do otherwise
        };

        let y_dim = (y.width, y.height);
        let c_dim = (u.width, u.height);
        let need_realloc = self
            .planes
            .as_ref()
            .map(|p| p.y_dim != y_dim || p.c_dim != c_dim)
            .unwrap_or(true);
        if need_realloc {
            let yt = plane_tex(device, "yuv-y", y_dim);
            let ut = plane_tex(device, "yuv-u", c_dim);
            let vt = plane_tex(device, "yuv-v", c_dim);
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("yuv-bg"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view(&yt)) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view(&ut)) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&view(&vt)) },
                    wgpu::BindGroupEntry { binding: 4, resource: self.uniform.as_entire_binding() },
                ],
            });
            self.planes = Some(Planes { y: yt, u: ut, v: vt, y_dim, c_dim, bind_group });
        }
        let planes = self.planes.as_ref().unwrap();

        upload_plane(queue, &planes.y, y);
        upload_plane(queue, &planes.u, u);
        upload_plane(queue, &planes.v, v);

        let (src_min, src_max, dst_min, dst_max) =
            fit_rects(frame.width, frame.height, canvas_size[0], canvas_size[1], fit);
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&Uniforms {
                dst_min,
                dst_max,
                src_min,
                src_max,
                full_range: if *full_range { 1.0 } else { 0.0 },
                bt709: if *bt709 { 1.0 } else { 0.0 },
                _pad: [0.0; 2],
            }),
        );
    }

    /// Record the YUV→RGB pass into `encoder`, writing `canvas_view` (must be a
    /// non-sRGB view so the matrix output is stored verbatim). Requires a prior
    /// `upload`; no-op if nothing has been uploaded yet.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, canvas_view: &TextureView) {
        let Some(planes) = self.planes.as_ref() else { return };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("yuv-convert-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: canvas_view,
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &planes.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn plane_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn plane_tex(device: &Device, label: &str, dim: (u32, u32)) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: dim.0.max(1), height: dim.1.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn view(tex: &wgpu::Texture) -> wgpu::TextureView {
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

fn upload_plane(queue: &Queue, tex: &wgpu::Texture, p: &crate::frame::YuvPlane) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &p.data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(p.stride),
            rows_per_image: Some(p.height),
        },
        wgpu::Extent3d { width: p.width, height: p.height, depth_or_array_layers: 1 },
    );
}

/// Source/dest UV rects for a frame placed on the canvas under `fit`. Mirrors the
/// CPU `compose_canvas` geometry, but expressed as normalized rects for the shader.
fn fit_rects(fw: u32, fh: u32, cw: u32, ch: u32, fit: CanvasFit) -> ([f32; 2], [f32; 2], [f32; 2], [f32; 2]) {
    let (fwf, fhf, cwf, chf) = (fw as f32, fh as f32, cw as f32, ch as f32);
    let full_src = ([0.0, 0.0], [1.0, 1.0]);
    let full_dst = ([0.0, 0.0], [1.0, 1.0]);
    if fw == 0 || fh == 0 || cw == 0 || ch == 0 {
        return (full_src.0, full_src.1, full_dst.0, full_dst.1);
    }
    match fit {
        CanvasFit::Stretch => (full_src.0, full_src.1, full_dst.0, full_dst.1),
        CanvasFit::Fit => {
            let s = (cwf / fwf).min(chf / fhf);
            let (w, h) = (fwf * s, fhf * s);
            let (x, y) = ((cwf - w) / 2.0, (chf - h) / 2.0);
            (full_src.0, full_src.1, [x / cwf, y / chf], [(x + w) / cwf, (y + h) / chf])
        }
        CanvasFit::Fill => {
            let s = (cwf / fwf).max(chf / fhf);
            let (vw, vh) = ((cwf / s).min(fwf), (chf / s).min(fhf));
            let (x, y) = ((fwf - vw) / 2.0, (fhf - vh) / 2.0);
            ([x / fwf, y / fhf], [(x + vw) / fwf, (y + vh) / fhf], full_dst.0, full_dst.1)
        }
    }
}

const SHADER: &str = r#"
struct Uniforms {
  dst_min: vec2<f32>,
  dst_max: vec2<f32>,
  src_min: vec2<f32>,
  src_max: vec2<f32>,
  full_range: f32,
  bt709: f32,
  _pad: vec2<f32>,
};
@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var tex_y: texture_2d<f32>;
@group(0) @binding(2) var tex_u: texture_2d<f32>;
@group(0) @binding(3) var tex_v: texture_2d<f32>;
@group(0) @binding(4) var<uniform> u: Uniforms;

struct VsOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
  // Fullscreen triangle; uv is canvas UV with (0,0) at top-left.
  let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
  var o: VsOut;
  o.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
  o.uv = vec2<f32>(p.x, 1.0 - p.y);
  return o;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
  // Letterbox: outside the dest rect stays black.
  if (in.uv.x < u.dst_min.x || in.uv.x > u.dst_max.x ||
      in.uv.y < u.dst_min.y || in.uv.y > u.dst_max.y) {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
  }
  let t = (in.uv - u.dst_min) / (u.dst_max - u.dst_min);
  let suv = u.src_min + t * (u.src_max - u.src_min);

  var yv = textureSampleLevel(tex_y, samp, suv, 0.0).r;
  var uu = textureSampleLevel(tex_u, samp, suv, 0.0).r;
  var vv = textureSampleLevel(tex_v, samp, suv, 0.0).r;

  if (u.full_range < 0.5) {
    yv = (yv - 0.0627451) * 1.1643836; // (Y-16/255)*255/219
    uu = (uu - 0.5019608) * 1.1383929; // (U-128/255)*255/224
    vv = (vv - 0.5019608) * 1.1383929;
  } else {
    uu = uu - 0.5019608;
    vv = vv - 0.5019608;
  }

  var rgb: vec3<f32>;
  if (u.bt709 > 0.5) {
    rgb = vec3<f32>(yv + 1.5748 * vv,
                    yv - 0.1873 * uu - 0.4681 * vv,
                    yv + 1.8556 * uu);
  } else {
    rgb = vec3<f32>(yv + 1.402 * vv,
                    yv - 0.344136 * uu - 0.714136 * vv,
                    yv + 1.772 * uu);
  }
  return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;
