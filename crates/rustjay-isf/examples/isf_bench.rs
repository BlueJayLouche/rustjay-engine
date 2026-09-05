//! Headless GPU timing harness for a single ISF shader.
//!
//! Renders the shader at 1280×720 RGBA8 for a fixed number of frames, forcing
//! GPU completion every frame (1-byte buffer map, same idiom as
//! tests/render_pixels.rs), and prints the average frame time as one JSON line:
//! `{"ms": <f64>, "frames": 60}`. Errors go to stderr with exit code 1.
//!
//! One shader per process on purpose: a pathological shader can hang the GPU
//! device, and process isolation keeps one bad shader from killing a batch.
//!
//! Run: cargo run --release -p rustjay-isf --example isf_bench -- <shader.fs>

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use rustjay_core::{EffectPlugin, EngineState, RenderHookCtx, Vertex};
use rustjay_isf::{IsfEffect, IsfState};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const WARMUP: u32 = 10;
const FRAMES: u32 = 60;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Engine-owned fullscreen-quad vertex buffer (RenderHookCtx requires one).
    quad_vb: wgpu::Buffer,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    /// 1-byte buffer mapped after every submit to force GPU completion.
    fence_buf: wgpu::Buffer,
}

fn init_gpu() -> Result<Gpu, String> {
    let (device, queue) = pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
                ..Default::default()
            })
            .await
            .map_err(|e| format!("no wgpu adapter: {e}"))?;
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                label: Some("ISF Bench Device"),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await
            .map_err(|e| format!("no wgpu device: {e}"))
    })?;
    let quad_vb = wgpu::util::DeviceExt::create_buffer_init(
        &device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("Bench Quad VB"),
            contents: bytemuck::cast_slice(&Vertex::quad_vertices()),
            usage: wgpu::BufferUsages::VERTEX,
        },
    );
    let format = rustjay_core::working_format();
    if !matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
    ) {
        return Err(format!("bench only supports 8-bit targets, got {format:?}"));
    }
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Bench Target"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let fence_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Bench Fence"),
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    Ok(Gpu {
        device,
        queue,
        quad_vb,
        target,
        target_view,
        fence_buf,
    })
}

/// Render one frame and block until the GPU has finished it.
/// TIME/TIMEDELTA come from IsfEffect's internal clock, so the loop itself
/// advances the animation (same as render_pixels.rs — no explicit time input).
fn render_frame(
    gpu: &Gpu,
    effect: &mut IsfEffect,
    state: &mut IsfState,
    engine: &EngineState,
) -> Result<(), String> {
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Bench Encoder"),
        });
    {
        let mut ctx = RenderHookCtx {
            encoder: &mut encoder,
            device: &gpu.device,
            queue: &gpu.queue,
            input: None,
            target_view: &gpu.target_view,
            engine_state: engine,
            vertex_buffer: &gpu.quad_vb,
        };
        if !effect.render(&mut ctx, state) {
            return Err("render() returned false".into());
        }
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &gpu.target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &gpu.fence_buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: None,
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit(std::iter::once(encoder.finish()));

    // ponytail: wall-clock around a forced poll is the naive timing ceiling —
    // it includes CPU submit overhead and says nothing about where GPU time
    // goes. Upgrade path: wgpu timestamp queries (Features::TIMESTAMP_QUERY).
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&done);
    gpu.fence_buf
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |res| {
            res.expect("map_async");
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
    while !done.load(std::sync::atomic::Ordering::SeqCst) {
        gpu.device.poll(wgpu::PollType::Poll).ok();
        std::thread::yield_now();
    }
    gpu.fence_buf.unmap();
    Ok(())
}

fn run() -> Result<(), String> {
    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: isf_bench <shader.fs>")?,
    );
    let gpu = init_gpu()?;

    let mut effect = IsfEffect::from_path(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    EffectPlugin::init(&mut effect, &gpu.device, &gpu.queue);
    if let Some(err) = &effect.transpile_error {
        return Err(format!("pipeline init failed: {err}"));
    }
    let mut state = effect.default_state();

    let mut engine = EngineState::new();
    engine.resolution.internal_width = WIDTH;
    engine.resolution.internal_height = HEIGHT;

    for _ in 0..WARMUP {
        render_frame(&gpu, &mut effect, &mut state, &engine)?;
    }
    let start = Instant::now();
    for _ in 0..FRAMES {
        render_frame(&gpu, &mut effect, &mut state, &engine)?;
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(FRAMES);
    println!("{{\"ms\": {ms:.3}, \"frames\": {FRAMES}}}");
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
