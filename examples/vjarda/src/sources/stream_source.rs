//! Live stream source — network ingest via ffmpeg (SRT / HLS / DASH / RTMP / RTMPS).
//!
//! Wraps `StreamDecoder` to pull decoded RGBA frames from a network URL and
//! upload them to a GPU texture each frame.  Unlike `FfmpegSource`, there is
//! no seeking, looping, or speed control — the stream is read continuously.

use rustjay_core::{
    EffectInput, EffectInstance, EngineState, ParamCategory, ParameterDescriptor, RenderCtx,
    RenderTarget,
};
use rustjay_io::StreamDecoder;

/// Renders live stream frames to the target.
pub struct StreamSource {
    decoder: StreamDecoder,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: Option<wgpu::BindGroup>,
    sampler: wgpu::Sampler,
    texture: Option<wgpu::Texture>,
    view: Option<wgpu::TextureView>,
    width: u32,
    height: u32,
    param_prefix: String,
    playing_key: String,
    last_playing: bool,
}

impl StreamSource {
    pub fn new(device: &wgpu::Device, _queue: &wgpu::Queue, url: &str) -> anyhow::Result<Self> {
        let decoder = StreamDecoder::new(url)?;
        let width = decoder.width();
        let height = decoder.height();

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("StreamSource Shader"),
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
            label: Some("StreamSource BGL"),
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
            label: Some("StreamSource Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("StreamSource Pipeline"),
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
                    format: rustjay_core::working_format(),
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
            playing_key: String::new(),
            last_playing: true,
        })
    }

    fn recompute_keys(&mut self) {
        let p = &self.param_prefix;
        self.playing_key = format!("{p}playing");
    }

    fn ensure_texture(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.texture.is_some() && self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("StreamSource Texture"),
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
            label: Some("StreamSource BG"),
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

impl EffectInstance for StreamSource {
    fn set_param_prefix(&mut self, prefix: &str) {
        self.param_prefix = prefix.to_string();
        self.recompute_keys();
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        let p = &self.param_prefix;
        vec![ParameterDescriptor::bool(
            format!("{p}playing"),
            "Playing",
            ParamCategory::Custom("Playback".to_string()),
            true,
        )]
    }

    fn prepare(&mut self, engine: &EngineState, device: &wgpu::Device, queue: &wgpu::Queue) {
        let playing = engine.get_param(&self.playing_key).unwrap_or(1.0) > 0.5;
        if playing != self.last_playing {
            // For now, just track state; actual pause/resume is a future
            // enhancement (would require reconnect or buffer management).
            self.last_playing = playing;
        }

        if !playing {
            return;
        }

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
                label: Some("StreamSource Pass"),
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
