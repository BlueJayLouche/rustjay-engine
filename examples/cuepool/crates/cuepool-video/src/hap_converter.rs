//! GPU-native HAP upload and conversion into CuePool's shared RGBA canvas.

use crate::frame::{FramePixels, VideoFrame};
use crate::yuv_converter::fit_rects;
use cuepool_core::CanvasFit;
use hap_parser::TextureFormat as HapFormat;
use wgpu::{Device, Queue, TextureFormat, TextureView};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    dst_min: [f32; 2],
    dst_max: [f32; 2],
    src_min: [f32; 2],
    src_max: [f32; 2],
    do_ycocg: f32,
    _pad: f32,
    logical_max: [f32; 2],
}

struct Binding {
    _texture: wgpu::Texture,
    dimensions: (u32, u32),
    format: HapFormat,
    bind_group: wgpu::BindGroup,
}

pub struct HapConverter {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    active: Option<Binding>,
}

impl HapConverter {
    pub fn new(device: &Device, target_format: TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hap-convert-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hap-convert-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
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
            label: Some("hap-convert-pl"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hap-convert"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hap-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hap-uniform"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            layout,
            sampler,
            uniform,
            active: None,
        }
    }

    pub fn upload(
        &mut self,
        device: &Device,
        queue: &Queue,
        frame: &VideoFrame,
        canvas_size: [u32; 2],
        fit: CanvasFit,
    ) -> Result<(), String> {
        let FramePixels::Hap {
            format,
            data,
            padded_width,
            padded_height,
        } = &frame.pixels
        else {
            return Err("HAP converter received a non-HAP frame".into());
        };
        validate_payload(*format, data, frame.width, frame.height)?;
        let texture_format = match format {
            HapFormat::RgbDxt1 => TextureFormat::Bc1RgbaUnorm,
            HapFormat::RgbaDxt5 | HapFormat::YcoCgDxt5 => TextureFormat::Bc3RgbaUnorm,
            other => return Err(format!("unsupported GPU-native HAP format: {other:?}")),
        };
        let dimensions = (*padded_width, *padded_height);
        validate_texture_dimensions(
            dimensions.0,
            dimensions.1,
            device.limits().max_texture_dimension_2d,
        )?;
        let recreate = self
            .active
            .as_ref()
            .is_none_or(|active| active.dimensions != dimensions || active.format != *format);
        if recreate {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("hap-bc-texture"),
                size: wgpu::Extent3d {
                    width: dimensions.0,
                    height: dimensions.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: texture_format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hap-convert-bg"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.uniform.as_entire_binding(),
                    },
                ],
            });
            self.active = Some(Binding {
                _texture: texture,
                dimensions,
                format: *format,
                bind_group,
            });
        }
        let texture = &self.active.as_ref().unwrap()._texture;
        let blocks_x = dimensions.0 / 4;
        let blocks_y = dimensions.1 / 4;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(blocks_x * format.bytes_per_block() as u32),
                rows_per_image: Some(blocks_y),
            },
            wgpu::Extent3d {
                width: dimensions.0,
                height: dimensions.1,
                depth_or_array_layers: 1,
            },
        );

        let (mut src_min, mut src_max, dst_min, dst_max) = fit_rects(
            frame.width,
            frame.height,
            canvas_size[0],
            canvas_size[1],
            fit,
        );
        let uv_scale = [
            frame.width as f32 / dimensions.0 as f32,
            frame.height as f32 / dimensions.1 as f32,
        ];
        for axis in 0..2 {
            src_min[axis] *= uv_scale[axis];
            src_max[axis] *= uv_scale[axis];
        }
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&Uniforms {
                dst_min,
                dst_max,
                src_min,
                src_max,
                do_ycocg: if format.needs_ycocg_convert() {
                    1.0
                } else {
                    0.0
                },
                _pad: 0.0,
                logical_max: uv_scale,
            }),
        );
        Ok(())
    }

    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, canvas_view: &TextureView) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hap-convert-pass"),
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
        pass.set_bind_group(0, &active.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn validate_payload(format: HapFormat, data: &[u8], width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("HAP frame has zero dimensions".into());
    }
    let expected = format.frame_size(width, height);
    if data.len() != expected {
        return Err(format!(
            "HAP frame size mismatch for {width}x{height} {format:?}: got {}, expected {expected}",
            data.len()
        ));
    }
    Ok(())
}

fn validate_texture_dimensions(width: u32, height: u32, max: u32) -> Result<(), String> {
    if width > max || height > max {
        return Err(format!(
            "HAP texture {width}x{height} exceeds the device limit {max}"
        ));
    }
    Ok(())
}

const SHADER: &str = r#"
struct Uniforms {
  dst_min: vec2<f32>,
  dst_max: vec2<f32>,
  src_min: vec2<f32>,
  src_max: vec2<f32>,
  do_ycocg: f32,
  _pad0: f32,
  logical_max: vec2<f32>,
};
@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> u: Uniforms;

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

fn hap_ycocg_to_rgb(c: vec4<f32>) -> vec3<f32> {
  let offset = 128.0 / 255.0;
  let scale = (c.b * (255.0 / 8.0)) + 1.0;
  let co = (c.r - offset) / scale;
  let cg = (c.g - offset) / scale;
  let y = c.a;
  return vec3<f32>(y + co - cg, y + cg, y - co - cg);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
  if (in.uv.x < u.dst_min.x || in.uv.x > u.dst_max.x ||
      in.uv.y < u.dst_min.y || in.uv.y > u.dst_max.y) {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
  }
  let t = (in.uv - u.dst_min) / (u.dst_max - u.dst_min);
  let texel = 0.5 / vec2<f32>(textureDimensions(tex));
  let uv = clamp(
    u.src_min + t * (u.src_max - u.src_min),
    texel,
    u.logical_max - texel,
  );
  let sample = textureSampleLevel(tex, samp, uv, 0.0);
  if (u.do_ycocg > 0.5) {
    return vec4<f32>(hap_ycocg_to_rgb(sample), 1.0);
  }
  return sample;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use hap_qt::{HapFormat as QtHapFormat, HapFrameEncoder};

    #[test]
    fn validates_block_padded_payload_sizes() {
        assert!(validate_payload(HapFormat::RgbDxt1, &[0; 32], 5, 5).is_ok());
        let error = validate_payload(HapFormat::RgbaDxt5, &[0; 63], 8, 8).unwrap_err();
        assert!(error.contains("got 63, expected 64"));
        assert!(validate_texture_dimensions(8_192, 4, 8_192).is_ok());
        assert!(validate_texture_dimensions(8_193, 4, 8_192).is_err());
    }

    fn render_hap(
        qt_format: QtHapFormat,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        canvas_size: [u32; 2],
    ) -> Option<(Vec<u8>, u32)> {
        let _gpu = crate::gpu_test_lock();
        let (device, queue) = crate::test_device_queue(wgpu::Features::TEXTURE_COMPRESSION_BC)?;
        let encoded = HapFrameEncoder::new(qt_format, width, height)
            .unwrap()
            .encode(&rgba)
            .unwrap();
        let parsed = hap_parser::parse_frame(&encoded).unwrap();
        let frame = VideoFrame::hap(width, height, 0.0, parsed.format, parsed.data);
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hap-test-target"),
            size: wgpu::Extent3d {
                width: canvas_size[0],
                height: canvas_size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut converter = HapConverter::new(&device, TextureFormat::Rgba8Unorm);
        converter
            .upload(&device, &queue, &frame, canvas_size, CanvasFit::Stretch)
            .unwrap();
        let bytes_per_row =
            (canvas_size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hap-test-readback"),
            size: u64::from(bytes_per_row * canvas_size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("hap-test-encoder"),
        });
        converter.encode(&mut encoder, &target.create_view(&Default::default()));
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(canvas_size[1]),
                },
            },
            wgpu::Extent3d {
                width: canvas_size[0],
                height: canvas_size[1],
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range().expect("mapped range").to_vec();
        readback.unmap();
        Some((data, bytes_per_row))
    }

    #[test]
    fn renders_hap_hap_alpha_and_hap_q_to_rgba() {
        for (format, expected_alpha) in [
            (QtHapFormat::Hap1, 255u8),
            (QtHapFormat::Hap5, 128u8),
            (QtHapFormat::HapY, 255u8),
        ] {
            let source = [200u8, 40, 20, expected_alpha].repeat(8 * 8);
            let Some((rendered, stride)) = render_hap(format, 8, 8, source, [8, 8]) else {
                return;
            };
            let offset = (4 * stride + 4 * 4) as usize;
            let pixel = &rendered[offset..offset + 4];
            for (actual, expected) in pixel[..3].iter().zip([200u8, 40, 20]) {
                assert!(
                    actual.abs_diff(expected) <= 40,
                    "{format:?} produced {pixel:?}"
                );
            }
            assert!(
                pixel[3].abs_diff(expected_alpha) <= 8,
                "{format:?} produced {pixel:?}"
            );
        }
    }

    #[test]
    fn crops_black_block_padding_for_non_multiple_of_four_frames() {
        let source = [220u8, 30, 20, 255].repeat(5 * 5);
        let Some((rendered, stride)) = render_hap(QtHapFormat::Hap1, 5, 5, source, [20, 20]) else {
            return;
        };
        let offset = (19 * stride + 19 * 4) as usize;
        assert!(
            rendered[offset] > 140,
            "bottom-right pixel was padded black"
        );
    }
}
