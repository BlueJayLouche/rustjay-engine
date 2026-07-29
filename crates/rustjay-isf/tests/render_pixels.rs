//! GPU pixel tests for the ISF pipeline — the phase-3 proof that shaders render
//! correctly, not just compile. Gated on `RUSTJAY_GPU_TESTS=1` (skip silently
//! otherwise so CI without a GPU stays green).
//!
//! Drives the real runtime: `IsfEffect::from_path` → `EffectPlugin::init` →
//! `render()` with a hand-constructed `RenderHookCtx`, then readback.
//!
//! Run: RUSTJAY_GPU_TESTS=1 cargo test -p rustjay-isf --test render_pixels -- --nocapture

use std::path::PathBuf;
use std::sync::Arc;

use rustjay_core::{EffectPlugin, EngineState, RenderHookCtx, Vertex};
use rustjay_isf::{IsfEffect, IsfState};

const EPS: u8 = 2;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn gpu_enabled() -> bool {
    std::env::var("RUSTJAY_GPU_TESTS").as_deref() == Ok("1")
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Engine-owned fullscreen-quad vertex buffer (RenderHookCtx requires one).
    quad_vb: wgpu::Buffer,
}

fn init_gpu() -> Option<Gpu> {
    if !gpu_enabled() {
        eprintln!("RUSTJAY_GPU_TESTS != 1 — skipping GPU pixel test");
        return None;
    }
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
            })
            .await
            .expect("no wgpu adapter");
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                label: Some("ISF Pixel Test Device"),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await
            .expect("no wgpu device")
    });
    let quad_vb = wgpu::util::DeviceExt::create_buffer_init(
        &device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("Test Quad VB"),
            contents: bytemuck::cast_slice(&Vertex::quad_vertices()),
            usage: wgpu::BufferUsages::VERTEX,
        },
    );
    Some(Gpu {
        device,
        queue,
        quad_vb,
    })
}

struct Frame {
    bytes_per_row: u32,
    format: wgpu::TextureFormat,
    data: Vec<u8>,
}

impl Frame {
    /// Pixel as (r, g, b, a), accounting for the target format's channel order.
    fn rgba(&self, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let i = (y * self.bytes_per_row + x * 4) as usize;
        let p = &self.data[i..i + 4];
        match self.format {
            wgpu::TextureFormat::Bgra8Unorm => (p[2], p[1], p[0], p[3]),
            _ => (p[0], p[1], p[2], p[3]), // Rgba8Unorm
        }
    }
}

fn engine_at(w: u32, h: u32) -> EngineState {
    let mut engine = EngineState::new();
    engine.resolution.internal_width = w;
    engine.resolution.internal_height = h;
    engine
}

/// Load + init an ISF shader from tests/shaders, render one frame, read back pixels.
fn render_shader(
    gpu: &Gpu,
    shader: &str,
    engine: &EngineState,
    state: &mut IsfState,
    input: Option<rustjay_core::EffectInput<'_>>,
    width: u32,
    height: u32,
) -> Frame {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/shaders")
        .join(shader);
    let mut effect = IsfEffect::from_path(&path).unwrap_or_else(|e| panic!("{shader}: {e}"));
    EffectPlugin::init(&mut effect, &gpu.device, &gpu.queue);
    if let Some(err) = &effect.transpile_error {
        panic!("{shader}: pipeline init failed: {err}");
    }

    let format = rustjay_core::working_format();
    assert!(
        matches!(
            format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
        ),
        "pixel tests only support 8-bit targets, got {format:?}"
    );
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Test Target"),
        size: wgpu::Extent3d {
            width,
            height,
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

    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Test Encoder"),
        });
    {
        let mut ctx = RenderHookCtx {
            encoder: &mut encoder,
            device: &gpu.device,
            queue: &gpu.queue,
            input,
            target_view: &target_view,
            engine_state: engine,
            vertex_buffer: &gpu.quad_vb,
        };
        assert!(effect.render(&mut ctx, state), "{shader}: render() = false");
    }

    let bytes_per_row = (width * 4).next_multiple_of(256);
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Test Readback"),
        size: bytes_per_row as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
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
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit(std::iter::once(encoder.finish()));

    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&done);
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |res| {
            res.expect("map_async");
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
    while !done.load(std::sync::atomic::Ordering::SeqCst) {
        gpu.device.poll(wgpu::PollType::Poll).ok();
        std::thread::yield_now();
    }
    let data = readback.slice(..).get_mapped_range().to_vec();
    Frame {
        bytes_per_row,
        format,
        data,
    }
}

fn load_effect(gpu: &Gpu, shader: &str) -> (IsfEffect, IsfState) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/shaders")
        .join(shader);
    let mut effect = IsfEffect::from_path(&path).unwrap_or_else(|e| panic!("{shader}: {e}"));
    EffectPlugin::init(&mut effect, &gpu.device, &gpu.queue);
    if let Some(err) = &effect.transpile_error {
        panic!("{shader}: pipeline init failed: {err}");
    }
    let state = effect.default_state();
    (effect, state)
}

fn assert_channel(actual: u8, expected: u8, what: &str) {
    assert!(
        actual.abs_diff(expected) <= EPS,
        "{what}: expected {expected}, got {actual}"
    );
}

/// A 2×2 RGBA input texture: TL red, TR green, BL blue, BR white.
fn input_texture_2x2(gpu: &Gpu) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Test Input 2x2"),
        size: wgpu::Extent3d {
            width: 2,
            height: 2,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[
            255, 0, 0, 255, // TL red
            0, 255, 0, 255, // TR green
            0, 0, 255, 255, // BL blue
            255, 255, 255, 255, // BR white
        ],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: Some(2),
        },
        wgpu::Extent3d {
            width: 2,
            height: 2,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    (texture, view, sampler)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// (a) Y-flip / geometry: vec4(isf_FragNormCoord, 0, 1). ISF is bottom-left origin,
/// so readback row 0 (texture top) must have green ≈ 1.0, last row green ≈ 0.0.
#[test]
fn a_normcoords_yflip() {
    let Some(gpu) = init_gpu() else { return };
    let engine = engine_at(64, 64);
    let (_effect, mut state) = load_effect(&gpu, "normcoords.fs");
    let f = render_shader(&gpu, "normcoords.fs", &engine, &mut state, None, 64, 64);

    let (r_tl, g_tl, _, a_tl) = f.rgba(0, 0);
    let (r_tr, g_tr, _, _) = f.rgba(63, 0);
    let (r_bl, g_bl, _, _) = f.rgba(0, 63);
    let (r_br, g_br, _, _) = f.rgba(63, 63);
    eprintln!("corners TL=({r_tl},{g_tl}) TR=({r_tr},{g_tr}) BL=({r_bl},{g_bl}) BR=({r_br},{g_br}) a={a_tl}");
    assert_channel(r_tl, 0, "top-left R");
    assert_channel(g_tl, 255, "top-left G (ISF y=1 at texture top)");
    assert_channel(r_tr, 255, "top-right R");
    assert_channel(g_tr, 255, "top-right G");
    assert_channel(r_bl, 0, "bottom-left R");
    assert_channel(g_bl, 0, "bottom-left G (ISF y=0 at texture bottom)");
    assert_channel(r_br, 255, "bottom-right R");
    assert_channel(g_br, 0, "bottom-right G");
    assert_channel(a_tl, 255, "alpha");
}

/// (b) Color input DEFAULT reaches the shader (was always-black before Phase 2).
#[test]
fn b_color_input_default() {
    let Some(gpu) = init_gpu() else { return };
    let engine = engine_at(64, 64);
    let (_e, mut state) = load_effect(&gpu, "colorinput.fs");
    let f = render_shader(&gpu, "colorinput.fs", &engine, &mut state, None, 64, 64);
    let (r, g, b, a) = f.rgba(32, 32);
    eprintln!("center pixel: ({r}, {g}, {b}, {a})");
    assert_channel(r, 255, "tint.r");
    assert_channel(g, 0, "tint.g");
    assert_channel(b, 128, "tint.b (0.5)");
    assert_channel(a, 255, "tint.a");
}

/// (c) Float param through the real engine param path (get_param), overriding
/// the state-seeded DEFAULT.
#[test]
fn c_float_param_via_engine() {
    let Some(gpu) = init_gpu() else { return };
    let (effect, mut state) = load_effect(&gpu, "floatparam.fs");
    let mut engine = engine_at(64, 64);
    // Register the plugin's parameters in the engine, then set v = 0.75.
    let descs = effect.parameters();
    engine.custom_param_bases = descs.iter().map(|d| d.default).collect();
    engine.custom_params = engine.custom_param_bases.clone();
    engine.param_descriptors = Arc::new(descs);
    engine.set_param_base("v", 0.75);
    assert_eq!(engine.get_param("v"), Some(0.75));

    let f = render_shader(&gpu, "floatparam.fs", &engine, &mut state, None, 64, 64);
    let (r, g, b, _) = f.rgba(32, 32);
    eprintln!("center pixel with v=0.75 via engine: ({r}, {g}, {b})");
    let expect = (0.75f32 * 255.0).round() as u8;
    assert_channel(r, expect, "v via engine (R)");
    assert_channel(g, expect, "v via engine (G)");
    assert_channel(b, expect, "v via engine (B)");

    // Fallback path: no engine param registered → state DEFAULT (0.25) wins.
    let engine2 = engine_at(64, 64);
    let f2 = render_shader(&gpu, "floatparam.fs", &engine2, &mut state, None, 64, 64);
    let (r2, _, _, _) = f2.rgba(32, 32);
    eprintln!("center pixel with state fallback: {r2}");
    assert_channel(r2, (0.25f32 * 255.0).round() as u8, "v via state DEFAULT");
}

/// (d) IMG_THIS_PIXEL passthrough: output must match input texels in the right
/// orientation (no vertical mirroring).
#[test]
fn d_img_this_pixel_passthrough() {
    let Some(gpu) = init_gpu() else { return };
    let (_tex, view, sampler) = input_texture_2x2(&gpu);
    let engine = engine_at(2, 2);
    let (_e, mut state) = load_effect(&gpu, "imgpassthrough.fs");
    let input = rustjay_core::EffectInput {
        view: &view,
        sampler: &sampler,
        generation: 0,
        texture: None,
    };
    let f = render_shader(
        &gpu,
        "imgpassthrough.fs",
        &engine,
        &mut state,
        Some(input),
        2,
        2,
    );
    let tl = f.rgba(0, 0);
    let tr = f.rgba(1, 0);
    let bl = f.rgba(0, 1);
    let br = f.rgba(1, 1);
    eprintln!("passthrough: TL={tl:?} TR={tr:?} BL={bl:?} BR={br:?}");
    assert_eq!(tl, (255, 0, 0, 255), "top-left must be red");
    assert_eq!(tr, (0, 255, 0, 255), "top-right must be green");
    assert_eq!(bl, (0, 0, 255, 255), "bottom-left must be blue");
    assert_eq!(br, (255, 255, 255, 255), "bottom-right must be white");
}

/// (e) gl_FragCoord wrapper: same corner expectations as (a) — the flipped
/// isf_FragCoord global must present ISF bottom-left coordinates.
#[test]
fn e_fragcoord_wrapper() {
    let Some(gpu) = init_gpu() else { return };
    let engine = engine_at(64, 64);
    let (_e, mut state) = load_effect(&gpu, "fragcoord.fs");
    let f = render_shader(&gpu, "fragcoord.fs", &engine, &mut state, None, 64, 64);
    let (r_tl, g_tl, _, _) = f.rgba(0, 0);
    let (r_br, g_br, _, _) = f.rgba(63, 63);
    eprintln!("fragcoord corners: TL=({r_tl},{g_tl}) BR=({r_br},{g_br})");
    assert_channel(r_tl, 0, "top-left R");
    assert_channel(g_tl, 255, "top-left G (flipped)");
    assert_channel(r_br, 255, "bottom-right R");
    assert_channel(g_br, 0, "bottom-right G");
}

/// (g) Shadertoy-style bare `mainImage` entry: the bridge must be synthesized,
/// with fragCoord in flipped ISF pixel coordinates (same corner expectations as (a)).
#[test]
fn g_mainimage_bridge() {
    let Some(gpu) = init_gpu() else { return };
    let engine = engine_at(64, 64);
    let (_e, mut state) = load_effect(&gpu, "mainimage.fs");
    let f = render_shader(&gpu, "mainimage.fs", &engine, &mut state, None, 64, 64);
    let (r_tl, g_tl, _, _) = f.rgba(0, 0);
    let (r_br, g_br, _, _) = f.rgba(63, 63);
    eprintln!("mainimage corners: TL=({r_tl},{g_tl}) BR=({r_br},{g_br})");
    assert_channel(r_tl, 0, "top-left R");
    assert_channel(g_tl, 255, "top-left G (flipped)");
    assert_channel(r_br, 255, "bottom-right R");
    assert_channel(g_br, 0, "bottom-right G");
}

/// (f) Official reference shader (nannou corpus Test-Color.fs): threshold filter
/// with color defaults. No input connected → black placeholder → below level →
/// highColor (white). Proves defaults render non-black AND filters render with
/// no upstream texture (old early-return-black removed). With a white input →
/// above level → lowColor (blue), additionally exercising IMG_THIS_PIXEL.
#[test]
fn f_test_color_reference() {
    let Some(gpu) = init_gpu() else { return };
    let engine = engine_at(64, 64);
    let (_e, mut state) = load_effect(&gpu, "Test-Color.fs");

    // No input: black placeholder, avg 0.0 <= level(0.5) → highColor = white.
    let f = render_shader(&gpu, "Test-Color.fs", &engine, &mut state, None, 64, 64);
    let px = f.rgba(32, 32);
    eprintln!("Test-Color no-input pixel: {px:?}");
    assert_eq!(px, (255, 255, 255, 255), "expected highColor (white)");

    // White 2×2 input: avg 1.0 > 0.5 → lowColor = blue.
    let (_tex, view, sampler) = input_texture_2x2(&gpu);
    let engine2 = engine_at(2, 2);
    let input = rustjay_core::EffectInput {
        view: &view,
        sampler: &sampler,
        generation: 0,
        texture: None,
    };
    let f2 = render_shader(&gpu, "Test-Color.fs", &engine2, &mut state, Some(input), 2, 2);
    // TL texel is red: avg 1/3 <= 0.5 → highColor (white).
    // BR texel is white: avg 1.0 > 0.5 → lowColor (blue).
    let red_px = f2.rgba(0, 0);
    let white_px = f2.rgba(1, 1);
    eprintln!("Test-Color red-texel pixel: {red_px:?}, white-texel pixel: {white_px:?}");
    assert_eq!(red_px, (255, 255, 255, 255), "red texel → highColor (white)");
    assert_eq!(white_px, (0, 0, 255, 255), "white texel → lowColor (blue)");
}
