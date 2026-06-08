//! ffmpeg video source — general video file playback via `rustjay-io`.
//!
//! Wraps `FfmpegDecoder` to decode `.mp4`/`.mkv`/`.avi`/`.webm` files and
//! upload RGBA frames to a GPU texture each frame. Playback parameters
//! (speed, loop, play/pause, position, in/out points) are exposed through
//! the engine param system.
//!
//! # Known limitations
//! - Synchronous software decode on the render thread. High-resolution or
//!   high-complexity files may drop frames.
//! - Seeking lands on the nearest keyframe and decodes forward.

use rustjay_core::{
    EffectInput, EffectInstance, EngineState, ParamCategory, ParameterDescriptor, RenderCtx,
    RenderTarget,
};
use rustjay_io::{FfmpegDecoder, LoopMode};
use std::path::Path;

/// Renders ffmpeg-decoded video frames to the target.
pub struct FfmpegSource {
    decoder: FfmpegDecoder,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: Option<wgpu::BindGroup>,
    sampler: wgpu::Sampler,
    texture: Option<wgpu::Texture>,
    view: Option<wgpu::TextureView>,
    width: u32,
    height: u32,
    param_prefix: String,
    speed_key: String,
    playing_key: String,
    loop_key: String,
    position_key: String,
    in_point_key: String,
    out_point_key: String,
    last_speed: f32,
    last_playing: bool,
    last_loop: i32,
    last_position: f32,
    last_in_point: f32,
    last_out_point: f32,
}

impl FfmpegSource {
    pub fn new(device: &wgpu::Device, _queue: &wgpu::Queue, path: &Path) -> anyhow::Result<Self> {
        let decoder = FfmpegDecoder::new(path)?;
        let width = decoder.width();
        let height = decoder.height();

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("FfmpegSource Shader"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
                struct VertexOutput {
                    @builtin(position) position: vec4<f32>,
                    @location(0) texcoord: vec2<f32>,
                };

                @group(0) @binding(0) var tex: texture_2d<f32>;
                @group(0) @binding(1) var sam: sampler;

                @vertex
                fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> VertexOutput {
                    var out: VertexOutput;
                    out.position = vec4<f32>(position, 0.0, 1.0);
                    out.texcoord = texcoord;
                    return out;
                }

                @fragment
                fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
                    return textureSample(tex, sam, in.texcoord);
                }
                "#
                .into(),
            ),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("FfmpegSource BGL"),
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("FfmpegSource Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("FfmpegSource Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[rustjay_core::Vertex::desc()],
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

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Ok(Self {
            decoder,
            pipeline,
            bind_group_layout,
            bind_group: None,
            sampler,
            texture: None,
            view: None,
            width,
            height,
            param_prefix: String::new(),
            speed_key: String::new(),
            playing_key: String::new(),
            loop_key: String::new(),
            position_key: String::new(),
            in_point_key: String::new(),
            out_point_key: String::new(),
            last_speed: 1.0,
            last_playing: true,
            last_loop: 1,
            last_position: 0.0,
            last_in_point: 0.0,
            last_out_point: 1.0,
        })
    }

    fn recompute_keys(&mut self) {
        let p = &self.param_prefix;
        self.speed_key = format!("{p}speed");
        self.playing_key = format!("{p}playing");
        self.loop_key = format!("{p}loop");
        self.position_key = format!("{p}position");
        self.in_point_key = format!("{p}in_point");
        self.out_point_key = format!("{p}out_point");
    }

    fn ensure_texture(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.texture.is_some() && self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Ffmpeg Source Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("FfmpegSource BG"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        }));
        self.texture = Some(texture);
        self.view = Some(view);
    }
}

impl EffectInstance for FfmpegSource {
    fn set_param_prefix(&mut self, prefix: &str) {
        self.param_prefix = prefix.to_string();
        self.recompute_keys();
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        let p = &self.param_prefix;
        vec![
            ParameterDescriptor::float(
                format!("{p}speed"),
                "Speed",
                ParamCategory::Custom("Playback".to_string()),
                0.0,
                2.0,
                1.0,
                0.01,
            ),
            ParameterDescriptor::bool(
                format!("{p}playing"),
                "Playing",
                ParamCategory::Custom("Playback".to_string()),
                true,
            ),
            ParameterDescriptor::enum_param(
                format!("{p}loop"),
                "Loop Mode",
                ParamCategory::Custom("Playback".to_string()),
                vec![
                    "None".to_string(),
                    "Loop".to_string(),
                    "PingPong".to_string(),
                ],
                1,
            ),
            ParameterDescriptor::float(
                format!("{p}position"),
                "Position",
                ParamCategory::Custom("Playback".to_string()),
                0.0,
                1.0,
                0.0,
                0.001,
            ),
            ParameterDescriptor::float(
                format!("{p}in_point"),
                "In Point",
                ParamCategory::Custom("Playback".to_string()),
                0.0,
                1.0,
                0.0,
                0.001,
            ),
            ParameterDescriptor::float(
                format!("{p}out_point"),
                "Out Point",
                ParamCategory::Custom("Playback".to_string()),
                0.0,
                1.0,
                1.0,
                0.001,
            ),
        ]
    }

    fn prepare(&mut self, engine: &EngineState, device: &wgpu::Device, queue: &wgpu::Queue) {
        // Sync playback params.
        let speed = engine.get_param(&self.speed_key).unwrap_or(1.0);
        if (speed - self.last_speed).abs() > f32::EPSILON {
            self.decoder.set_speed(speed);
            self.last_speed = speed;
        }

        let playing = engine.get_param(&self.playing_key).unwrap_or(1.0) > 0.5;
        if playing != self.last_playing {
            if playing {
                self.decoder.play();
            } else {
                self.decoder.pause();
            }
            self.last_playing = playing;
        }

        let loop_raw = engine.get_param(&self.loop_key).unwrap_or(1.0) as i32;
        if loop_raw != self.last_loop {
            let mode = match loop_raw {
                0 => LoopMode::None,
                2 => LoopMode::PingPong,
                _ => LoopMode::Loop,
            };
            self.decoder.set_loop_mode(mode);
            self.last_loop = loop_raw;
        }

        let position = engine.get_param(&self.position_key).unwrap_or(0.0);
        if (position - self.last_position).abs() > 0.001 {
            self.decoder.seek_to(position as f64);
            self.last_position = position;
        }

        let in_point = engine.get_param(&self.in_point_key).unwrap_or(0.0);
        if (in_point - self.last_in_point).abs() > 0.001 {
            let t = in_point as f64 * self.decoder.duration();
            self.decoder.set_in_point(t);
            self.last_in_point = in_point;
        }

        let out_point = engine.get_param(&self.out_point_key).unwrap_or(1.0);
        if (out_point - self.last_out_point).abs() > 0.001 {
            let t = out_point as f64 * self.decoder.duration();
            self.decoder.set_out_point(t);
            self.last_out_point = out_point;
        }

        // Decode and upload.
        if let Some(frame) = self.decoder.decode_frame() {
            self.ensure_texture(device, frame.width, frame.height);
            if let Some(ref texture) = self.texture {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &frame.data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(frame.width * 4),
                        rows_per_image: Some(frame.height),
                    },
                    wgpu::Extent3d {
                        width: frame.width,
                        height: frame.height,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
    }

    fn render_to(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        _inputs: &[EffectInput<'_>],
        target: RenderTarget<'_>,
        _engine: &EngineState,
    ) {
        if let Some(ref bind_group) = self.bind_group {
            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("FfmpegSource Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, ctx.vertex_buffer.slice(..));
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}
