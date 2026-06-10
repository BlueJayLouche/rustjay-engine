//! DeckLink video source — implements [`EffectInstance`] by capturing frames
//! from a Blackmagic Design DeckLink device via a minimal Windows COM wrapper.

use rustjay_core::{EffectInput, EffectInstance, EngineState, RenderCtx, RenderTarget};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

extern "C" {
    fn decklink_init(device_index: i32) -> i32;
    fn decklink_get_frame(
        width: *mut i32,
        height: *mut i32,
        row_bytes: *mut i32,
        data: *mut *mut u8,
    ) -> i32;
    fn decklink_shutdown();
}

/// Frame data sent from the DeckLink capture thread to the render thread.
struct FrameData {
    width: u32,
    height: u32,
    row_bytes: u32,
    data: Vec<u8>,
}

/// Polls the C++ wrapper for new frames in a background thread.
fn capture_thread(sender: crossbeam::channel::Sender<FrameData>, stop: Arc<AtomicBool>) {
    let result = unsafe { decklink_init(0) };
    if result != 0 {
        log::error!("DeckLink capture thread: init failed with code {}", result);
        return;
    }

    log::info!("DeckLink: capture started");

    while !stop.load(Ordering::Relaxed) {
        let mut width = 0i32;
        let mut height = 0i32;
        let mut row_bytes = 0i32;
        let mut data: *mut u8 = std::ptr::null_mut();

        let got_frame = unsafe { decklink_get_frame(&mut width, &mut height, &mut row_bytes, &mut data) };

        if got_frame == 1 && !data.is_null() {
            let size = (row_bytes as usize) * (height as usize);
            let frame_data = unsafe { Vec::from_raw_parts(data, size, size) };

            let _ = sender.try_send(FrameData {
                width: width as u32,
                height: height as u32,
                row_bytes: row_bytes as u32,
                data: frame_data,
            });
        }

        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    unsafe { decklink_shutdown() };
    log::info!("DeckLink: capture stopped");
}

/// Renders live DeckLink frames to the target.
pub struct DecklinkSource {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: Option<wgpu::BindGroup>,
    texture: Option<wgpu::Texture>,
    view: Option<wgpu::TextureView>,
    sampler: wgpu::Sampler,
    width: u32,
    height: u32,
    receiver: crossbeam::channel::Receiver<FrameData>,
    stop_flag: Arc<AtomicBool>,
}

impl DecklinkSource {
    pub fn new(device: &wgpu::Device, _device_index: usize) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("DecklinkSource Shader"),
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
            label: Some("DecklinkSource BGL"),
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
            label: Some("DecklinkSource Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("DecklinkSource Pipeline"),
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

        let (sender, receiver) = crossbeam::channel::bounded(1);
        let stop_flag = Arc::new(AtomicBool::new(false));

        let stop = Arc::clone(&stop_flag);
        std::thread::spawn(move || {
            capture_thread(sender, stop);
        });

        Self {
            pipeline,
            bind_group_layout,
            bind_group: None,
            texture: None,
            view: None,
            sampler,
            width: 1920,
            height: 1080,
            receiver,
            stop_flag,
        }
    }

    fn ensure_texture(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.texture.is_some() && self.width == width && self.height == height {
            return;
        }

        self.width = width;
        self.height = height;

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Decklink Source Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("DecklinkSource BG"),
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
        });

        self.texture = Some(texture);
        self.view = Some(view);
        self.bind_group = Some(bind_group);
    }
}

impl Drop for DecklinkSource {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

impl EffectInstance for DecklinkSource {
    fn prepare(&mut self, _engine: &EngineState, device: &wgpu::Device, queue: &wgpu::Queue) {
        while let Ok(frame) = self.receiver.try_recv() {
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
                        bytes_per_row: Some(frame.row_bytes),
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
                label: Some("DecklinkSource Pass"),
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
