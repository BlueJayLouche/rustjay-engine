//! GPU YUV → RGB conversion. Uploads decoded planes as R8/R16/Rg8 textures and
//! runs one fullscreen pass that does the colorspace matrix + canvas fit,
//! writing the projection canvas. 4:2:0/4:2:2/4:4:4 planar (8 and 10-bit) and
//! NV12 are handled natively; everything else is converted to RGBA upstream by
//! swscale.

use crate::frame::{BitDepth, FramePixels, VideoFrame};
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
    bit_depth_scale: f32,
    _pad: f32,
}

struct PlanarBinding {
    y: wgpu::Texture,
    u: wgpu::Texture,
    v: wgpu::Texture,
    y_dim: (u32, u32),
    c_dim: (u32, u32),
    bit_depth: BitDepth,
    bind_group: wgpu::BindGroup,
}

struct Nv12Binding {
    y: wgpu::Texture,
    uv: wgpu::Texture,
    y_dim: (u32, u32),
    uv_dim: (u32, u32),
    bind_group: wgpu::BindGroup,
}

enum ActiveBinding {
    Planar(PlanarBinding),
    Nv12(Nv12Binding),
}

pub struct YuvConverter {
    planar_pipeline: wgpu::RenderPipeline,
    planar_layout: wgpu::BindGroupLayout,
    nv12_pipeline: wgpu::RenderPipeline,
    nv12_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    active: Option<ActiveBinding>,
}

impl YuvConverter {
    pub fn new(device: &Device, target_format: TextureFormat) -> Self {
        let planar_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv-planar-shader"),
            source: wgpu::ShaderSource::Wgsl(PLANAR_SHADER.into()),
        });
        let nv12_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv-nv12-shader"),
            source: wgpu::ShaderSource::Wgsl(NV12_SHADER.into()),
        });

        let planar_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuv-planar-bgl"),
            entries: &[
                sampler_entry(0),
                plane_entry(1),
                plane_entry(2),
                plane_entry(3),
                uniform_entry(4),
            ],
        });

        let nv12_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuv-nv12-bgl"),
            entries: &[
                sampler_entry(0),
                plane_entry(1),
                plane_entry(2),
                uniform_entry(3),
            ],
        });

        let planar_pipeline = create_pipeline(device, &planar_layout, &planar_shader, target_format, "yuv-planar");
        let nv12_pipeline = create_pipeline(device, &nv12_layout, &nv12_shader, target_format, "yuv-nv12");

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

        Self { planar_pipeline, planar_layout, nv12_pipeline, nv12_layout, sampler, uniform, active: None }
    }

    /// Upload `frame`'s planes + the fit/colourspace uniform. Pairs with `encode`,
    /// which records the conversion pass (split so the pass can be folded into the
    /// first output's command encoder).
    pub fn upload(
        &mut self,
        device: &Device,
        queue: &Queue,
        frame: &VideoFrame,
        canvas_size: [u32; 2],
        fit: CanvasFit,
    ) {
        let (src_min, src_max, dst_min, dst_max) =
            fit_rects(frame.width, frame.height, canvas_size[0], canvas_size[1], fit);

        match &frame.pixels {
            FramePixels::YuvPlanar { subsample: _, bit_depth, y, u, v, full_range, bt709 } => {
                let y_dim = (y.width, y.height);
                let c_dim = (u.width, u.height);
                let need_realloc = self
                    .active
                    .as_ref()
                    .map(|a| match a {
                        ActiveBinding::Planar(p) => {
                            p.y_dim != y_dim || p.c_dim != c_dim || p.bit_depth != *bit_depth
                        }
                        _ => true,
                    })
                    .unwrap_or(true);
                let tex_format = match bit_depth {
                    BitDepth::B8 => TextureFormat::R8Unorm,
                    BitDepth::B10 => TextureFormat::R16Unorm,
                };
                if need_realloc {
                    let yt = plane_tex(device, "yuv-y", y_dim, tex_format);
                    let ut = plane_tex(device, "yuv-u", c_dim, tex_format);
                    let vt = plane_tex(device, "yuv-v", c_dim, tex_format);
                    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("yuv-planar-bg"),
                        layout: &self.planar_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view(&yt)) },
                            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view(&ut)) },
                            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&view(&vt)) },
                            wgpu::BindGroupEntry { binding: 4, resource: self.uniform.as_entire_binding() },
                        ],
                    });
                    self.active = Some(ActiveBinding::Planar(PlanarBinding {
                        y: yt,
                        u: ut,
                        v: vt,
                        y_dim,
                        c_dim,
                        bit_depth: *bit_depth,
                        bind_group,
                    }));
                }
                let ActiveBinding::Planar(binding) = self.active.as_ref().unwrap() else { unreachable!() };
                upload_plane(queue, &binding.y, y);
                upload_plane(queue, &binding.u, u);
                upload_plane(queue, &binding.v, v);

                let bit_depth_scale = match bit_depth {
                    BitDepth::B8 => 1.0,
                    BitDepth::B10 => 65535.0 / 1023.0,
                };
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
                        bit_depth_scale,
                        _pad: 0.0,
                    }),
                );
            }
            FramePixels::Nv12 { y, uv, full_range, bt709 } => {
                let y_dim = (y.width, y.height);
                let uv_dim = (uv.width, uv.height);
                let need_realloc = self
                    .active
                    .as_ref()
                    .map(|a| match a {
                        ActiveBinding::Nv12(n) => n.y_dim != y_dim || n.uv_dim != uv_dim,
                        _ => true,
                    })
                    .unwrap_or(true);
                if need_realloc {
                    let yt = plane_tex(device, "yuv-y", y_dim, TextureFormat::R8Unorm);
                    let uvt = plane_tex(device, "yuv-uv", uv_dim, TextureFormat::Rg8Unorm);
                    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("yuv-nv12-bg"),
                        layout: &self.nv12_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view(&yt)) },
                            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view(&uvt)) },
                            wgpu::BindGroupEntry { binding: 3, resource: self.uniform.as_entire_binding() },
                        ],
                    });
                    self.active = Some(ActiveBinding::Nv12(Nv12Binding {
                        y: yt,
                        uv: uvt,
                        y_dim,
                        uv_dim,
                        bind_group,
                    }));
                }
                let ActiveBinding::Nv12(binding) = self.active.as_ref().unwrap() else { unreachable!() };
                upload_plane(queue, &binding.y, y);
                upload_plane(queue, &binding.uv, uv);

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
                        bit_depth_scale: 1.0,
                        _pad: 0.0,
                    }),
                );
            }
            FramePixels::Rgba(_) => {} // caller shouldn't reach here, but safe no-op
        }
    }

    /// Record the YUV→RGB pass into `encoder`, writing `canvas_view` (must be a
    /// non-sRGB view so the matrix output is stored verbatim). Requires a prior
    /// `upload`; no-op if nothing has been uploaded yet.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, canvas_view: &TextureView) {
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

        match self.active.as_ref() {
            Some(ActiveBinding::Planar(binding)) => {
                pass.set_pipeline(&self.planar_pipeline);
                pass.set_bind_group(0, &binding.bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            Some(ActiveBinding::Nv12(binding)) => {
                pass.set_pipeline(&self.nv12_pipeline);
                pass.set_bind_group(0, &binding.bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            None => {}
        }
    }
}

fn create_pipeline(
    device: &Device,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    target_format: TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(&format!("{label}-pl")),
        bind_group_layouts: &[Some(layout)],
        ..Default::default()
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pl),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
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
    })
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
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

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn plane_tex(device: &Device, label: &str, dim: (u32, u32), format: TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: dim.0.max(1), height: dim.1.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
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

const PLANAR_SHADER: &str = r#"
struct Uniforms {
  dst_min: vec2<f32>,
  dst_max: vec2<f32>,
  src_min: vec2<f32>,
  src_max: vec2<f32>,
  full_range: f32,
  bt709: f32,
  bit_depth_scale: f32,
  _pad: f32,
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

  var yv = textureSampleLevel(tex_y, samp, suv, 0.0).r * u.bit_depth_scale;
  var uu = textureSampleLevel(tex_u, samp, suv, 0.0).r * u.bit_depth_scale;
  var vv = textureSampleLevel(tex_v, samp, suv, 0.0).r * u.bit_depth_scale;

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

const NV12_SHADER: &str = r#"
struct Uniforms {
  dst_min: vec2<f32>,
  dst_max: vec2<f32>,
  src_min: vec2<f32>,
  src_max: vec2<f32>,
  full_range: f32,
  bt709: f32,
  bit_depth_scale: f32,
  _pad: f32,
};
@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var tex_y: texture_2d<f32>;
@group(0) @binding(2) var tex_uv: texture_2d<f32>;
@group(0) @binding(3) var<uniform> u: Uniforms;

struct VsOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
  let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
  var o: VsOut;
  o.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
  o.uv = vec2<f32>(p.x, 1.0 - p.y);
  return o;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
  if (in.uv.x < u.dst_min.x || in.uv.x > u.dst_max.x ||
      in.uv.y < u.dst_min.y || in.uv.y > u.dst_max.y) {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
  }
  let t = (in.uv - u.dst_min) / (u.dst_max - u.dst_min);
  let suv = u.src_min + t * (u.src_max - u.src_min);

  var yv = textureSampleLevel(tex_y, samp, suv, 0.0).r;
  let uv = textureSampleLevel(tex_uv, samp, suv, 0.0).rg;
  var uu = uv.x;
  var vv = uv.y;

  if (u.full_range < 0.5) {
    yv = (yv - 0.0627451) * 1.1643836;
    uu = (uu - 0.5019608) * 1.1383929;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{BitDepth, ChromaSubsample, YuvPlane};

    fn fake_device_queue() -> (Device, Queue) {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_features: wgpu::Features::TEXTURE_FORMAT_16BIT_NORM,
                    ..Default::default()
                })
                .await
                .expect("device");
            (device, queue)
        })
    }

    fn plane(data: Vec<u8>, width: u32, height: u32) -> YuvPlane {
        let stride = data.len() as u32 / height.max(1);
        YuvPlane { data, stride, width, height }
    }

    #[test]
    fn planar_converter_compiles_and_uploads() {
        let (device, queue) = fake_device_queue();
        let mut conv = YuvConverter::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        let frame = VideoFrame::yuv_planar(
            4,
            4,
            0.0,
            ChromaSubsample::Cs420,
            BitDepth::B8,
            plane(vec![128u8; 4 * 4], 4, 4),
            plane(vec![128u8; 2 * 2], 2, 2),
            plane(vec![128u8; 2 * 2], 2, 2),
            false,
            true,
        );
        conv.upload(&device, &queue, &frame, [8, 8], CanvasFit::Fit);
    }

    #[test]
    fn nv12_converter_compiles_and_uploads() {
        let (device, queue) = fake_device_queue();
        let mut conv = YuvConverter::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        let frame = VideoFrame::nv12(
            4,
            4,
            0.0,
            plane(vec![128u8; 4 * 4], 4, 4),
            plane(vec![128u8; 2 * 2 * 2], 2, 2), // Rg8Unorm: 2 bytes per UV texel
            false,
            true,
        );
        conv.upload(&device, &queue, &frame, [8, 8], CanvasFit::Fit);
    }

    #[test]
    fn p10_converter_compiles_and_uploads() {
        let (device, queue) = fake_device_queue();
        let mut conv = YuvConverter::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        // 10-bit samples stored as little-endian u16 (two bytes each).
        let y = (0..(4 * 4))
            .flat_map(|_| (512u16).to_le_bytes())
            .collect::<Vec<u8>>();
        let uv = (0..(2 * 2))
            .flat_map(|_| (512u16).to_le_bytes())
            .collect::<Vec<u8>>();
        let frame = VideoFrame::yuv_planar(
            4,
            4,
            0.0,
            ChromaSubsample::Cs420,
            BitDepth::B10,
            plane(y, 4, 4),
            plane(uv.clone(), 2, 2),
            plane(uv, 2, 2),
            false,
            true,
        );
        conv.upload(&device, &queue, &frame, [8, 8], CanvasFit::Fit);
    }
}
