//! Camera source — captures from a webcam via `rustjay-io` and uploads frames
//! to a GPU texture each frame.
//!
//! One physical camera is shared across all `CameraSource` instances with the
//! same `device_index` (T02.3b). The first deck opens the session; subsequent
//! decks join it. Frames are cached in an `Arc<Vec<u8>>` so each deck can
//! upload to its own texture without per-frame cloning.

use rustjay_core::{EffectInput, EffectInstance, EngineState, RenderCtx, RenderTarget};
use rustjay_io::InputManager;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

struct CameraSession {
    manager: InputManager,
    frame: Option<Arc<Vec<u8>>>,
    resolution: (u32, u32),
}

static CAMERA_SESSIONS: OnceLock<Mutex<HashMap<usize, Arc<Mutex<CameraSession>>>>> =
    OnceLock::new();

fn get_session(device_index: usize) -> Arc<Mutex<CameraSession>> {
    let map = CAMERA_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap();
    guard
        .entry(device_index)
        .or_insert_with(|| {
            Arc::new(Mutex::new(CameraSession {
                manager: InputManager::new(),
                frame: None,
                resolution: (1280, 720),
            }))
        })
        .clone()
}

/// Renders live webcam frames to the target.
pub struct CameraSource {
    session: Arc<Mutex<CameraSession>>,
    device_index: usize,
    started: bool,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: Option<wgpu::BindGroup>,
    texture: Option<wgpu::Texture>,
    view: Option<wgpu::TextureView>,
    sampler: wgpu::Sampler,
    width: u32,
    height: u32,
}

impl CameraSource {
    pub fn new(device: &wgpu::Device, device_index: usize) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("CameraSource Shader"),
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
            label: Some("CameraSource BGL"),
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
            label: Some("CameraSource Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("CameraSource Pipeline"),
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

        Self {
            session: get_session(device_index),
            device_index,
            started: false,
            pipeline,
            bind_group_layout,
            bind_group: None,
            texture: None,
            view: None,
            sampler,
            width: 1280,
            height: 720,
        }
    }

    fn ensure_texture(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.texture.is_some() && self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Camera Source Texture"),
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
        self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("CameraSource BG"),
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

impl Drop for CameraSource {
    fn drop(&mut self) {
        let map = CAMERA_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()));
        let Ok(mut guard) = map.lock() else { return };
        if let Some(entry) = guard.get(&self.device_index) {
            // If strong_count is 2, only this CameraSource and the map hold the Arc.
            // Remove from map so the session is dropped and the webcam stops.
            if Arc::strong_count(entry) <= 2 {
                if let Some(session) = guard.remove(&self.device_index) {
                    if let Ok(mut s) = session.lock() {
                        s.manager.stop();
                        log::info!("CameraSource stopped webcam {}", self.device_index);
                    }
                }
            }
        }
    }
}

impl EffectInstance for CameraSource {
    fn prepare(&mut self, _engine: &EngineState, _device: &wgpu::Device, _queue: &wgpu::Queue) {
        let mut session = self.session.lock().unwrap();
        if !self.started {
            if !session.manager.is_active() {
                match session
                        .manager
                        .start_webcam(self.device_index, self.width, self.height, 30)
                { Err(e) => {
                    log::warn!(
                        "CameraSource failed to start webcam {}: {}",
                        self.device_index,
                        e
                    );
                } _ => {
                    self.started = true;
                    log::info!("CameraSource started webcam {}", self.device_index);
                }}
            } else {
                self.started = true;
                log::info!("CameraSource joined existing webcam {}", self.device_index);
            }
        }
        // Discover devices in background (nokhwa may need this on some platforms)
        if !session.manager.is_discovering() {
            session.manager.begin_refresh_devices();
        }
        let _ = session.manager.poll_discovery();
    }

    fn render_to(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        _inputs: &[EffectInput<'_>],
        target: RenderTarget<'_>,
        _engine: &EngineState,
    ) {
        if !self.started {
            return;
        }

        {
            let mut session = self.session.lock().unwrap();
            session.manager.update();
            if let Some(frame) = session.manager.take_frame() {
                session.resolution = session.manager.resolution();
                session.frame = Some(Arc::new(frame));
            }

            let frame_arc: Option<Arc<Vec<u8>>> = session.frame.clone();
            let (w, h) = session.resolution;
            drop(session);

            if let Some(frame) = frame_arc {
                self.ensure_texture(ctx.device, w, h);
                if let Some(ref texture) = self.texture {
                    ctx.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &frame,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(w * 4),
                            rows_per_image: Some(h),
                        },
                        wgpu::Extent3d {
                            width: w,
                            height: h,
                            depth_or_array_layers: 1,
                        },
                    );
                }
            }
        }

        if let Some(ref bind_group) = self.bind_group {
            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("CameraSource Pass"),
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
