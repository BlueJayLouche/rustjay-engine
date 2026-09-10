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
use wgpu::util::DeviceExt as _;
use rustjay_isf::{IsfEffect, IsfState};

/// Render size. Overridable so a shader can be timed at the reduced resolution
/// a quality setting would actually give it.
static WIDTH: std::sync::LazyLock<u32> =
    std::sync::LazyLock::new(|| env_u32("ISF_BENCH_WIDTH", 1280));
static HEIGHT: std::sync::LazyLock<u32> =
    std::sync::LazyLock::new(|| env_u32("ISF_BENCH_HEIGHT", 720));

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}
const WARMUP: u32 = 10;
const FRAMES: u32 = 60;
/// Thumbnail size. Overridable so a render can be inspected at 1:1.
static THUMB_W: std::sync::LazyLock<u32> =
    std::sync::LazyLock::new(|| env_u32("ISF_BENCH_THUMB_W", 320));
static THUMB_H: std::sync::LazyLock<u32> =
    std::sync::LazyLock::new(|| env_u32("ISF_BENCH_THUMB_H", 180));

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Engine-owned fullscreen-quad vertex buffer (RenderHookCtx requires one).
    quad_vb: wgpu::Buffer,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    /// 1-byte buffer mapped after every submit to force GPU completion.
    fence_buf: wgpu::Buffer,
    /// Full-frame readback, only used when a thumbnail is requested.
    readback: wgpu::Buffer,
    /// Test pattern bound as the input image, so effects have something to eat.
    input_view: wgpu::TextureView,
    input_sampler: wgpu::Sampler,
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
            width: *WIDTH,
            height: *HEIGHT,
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
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(*WIDTH * *HEIGHT * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    // ponytail: a procedural colour/checker pattern rather than a bundled image
    // — an effect only needs *something* with edges and colour to show what it
    // does. Swap in a real photo if a shader ever needs plausible content.
    let mut texels = Vec::with_capacity((*WIDTH * *HEIGHT * 4) as usize);
    for y in 0..*HEIGHT {
        for x in 0..*WIDTH {
            let check = ((x / 80) + (y / 80)) % 2 == 0;
            let fx = (x * 255 / *WIDTH) as u8;
            let fy = (y * 255 / *HEIGHT) as u8;
            let k = if check { 255 } else { 90 };
            texels.extend_from_slice(&[fx.max(k / 3), fy.max(k / 4), k, 255]);
        }
    }
    let input_tex = device.create_texture_with_data(
        &queue,
        &wgpu::TextureDescriptor {
            label: Some("test pattern"),
            size: wgpu::Extent3d { width: *WIDTH, height: *HEIGHT, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &texels,
    );
    let input_view = input_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let input_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("test pattern sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    Ok(Gpu {
        device,
        queue,
        quad_vb,
        target,
        target_view,
        fence_buf,
        readback,
        input_view,
        input_sampler,
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
            input: Some(rustjay_core::EffectInput {
                view: &gpu.input_view,
                sampler: &gpu.input_sampler,
                generation: 0,
                texture: None,
            }),
            input_b: None,
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

/// Copy the last rendered frame back and write it as a downscaled PNG.
/// ponytail: full-res readback then downscale, rather than rendering a second
/// small pass — one extra copy per shader is cheaper than a second pipeline.
fn save_png(gpu: &Gpu, path: &std::path::Path) -> Result<(), String> {
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("thumb") });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &gpu.target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &gpu.readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                // 1280 * 4 = 5120, already a multiple of the 256-byte alignment.
                bytes_per_row: Some(*WIDTH * 4),
                rows_per_image: Some(*HEIGHT),
            },
        },
        wgpu::Extent3d {
            width: *WIDTH,
            height: *HEIGHT,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit(std::iter::once(encoder.finish()));

    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&done);
    gpu.readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |res| {
            res.expect("map_async");
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
    while !done.load(std::sync::atomic::Ordering::SeqCst) {
        gpu.device.poll(wgpu::PollType::Poll).ok();
        std::thread::yield_now();
    }
    let img = {
        let data = gpu
            .readback
            .slice(..)
            .get_mapped_range()
            .map_err(|e| format!("map range: {e}"))?;
        image::RgbaImage::from_raw(*WIDTH, *HEIGHT, data.to_vec())
            .ok_or("readback buffer wrong size")?
    };
    gpu.readback.unmap();
    image::imageops::thumbnail(&img, *THUMB_W, *THUMB_H)
        .save(path)
        .map_err(|e| format!("save {}: {e}", path.display()))
}

fn run() -> Result<(), String> {
    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: isf_bench <shader.fs> [thumb.png]")?,
    );
    let thumb = std::env::args().nth(2).map(PathBuf::from);
    let gpu = init_gpu()?;

    let mut effect = IsfEffect::from_path(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    // A fixed step makes the render reproducible, so two builds of the
    // transpiler (or a shader before and after a rewrite) can be compared.
    effect.fixed_delta = Some(1.0 / 60.0);
    EffectPlugin::init(&mut effect, &gpu.device, &gpu.queue);
    if let Some(err) = &effect.transpile_error {
        return Err(format!("pipeline init failed: {err}"));
    }
    let mut state = effect.default_state();

    let mut engine = EngineState::new();
    engine.resolution.internal_width = *WIDTH;
    engine.resolution.internal_height = *HEIGHT;

    for _ in 0..WARMUP {
        render_frame(&gpu, &mut effect, &mut state, &engine)?;
    }
    let start = Instant::now();
    for _ in 0..FRAMES {
        render_frame(&gpu, &mut effect, &mut state, &engine)?;
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(FRAMES);
    if let Some(t) = &thumb {
        save_png(&gpu, t)?;
    }
    println!("{{\"ms\": {ms:.3}, \"frames\": {FRAMES}}}");
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
