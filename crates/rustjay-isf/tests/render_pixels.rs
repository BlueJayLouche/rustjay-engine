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
                ..Default::default()
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
    let (mut effect, _) = load_effect(gpu, shader);
    render_loaded(
        gpu,
        &mut effect,
        shader,
        engine,
        state,
        input,
        None,
        (width, height),
    )
}

/// The same render against an effect that is already loaded, so successive
/// frames see the state the last one left — which is the only way to watch
/// anything the runtime integrates over time.
// A harness that hands a shader everything the engine would: two inputs, a
// size, and both halves of the effect's state.
#[allow(clippy::too_many_arguments)]
fn render_loaded(
    gpu: &Gpu,
    effect: &mut IsfEffect,
    shader: &str,
    engine: &EngineState,
    state: &mut IsfState,
    input: Option<rustjay_core::EffectInput<'_>>,
    input_b: Option<rustjay_core::EffectInput<'_>>,
    (width, height): (u32, u32),
) -> Frame {
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
            input_b,
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
    let data = readback.slice(..).get_mapped_range().expect("buffer mapped by map_async").to_vec();
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

/// (h) MadMapper material entry point. MadMapper's own porting note says a
/// material is an ISF `main()` with `texCoord` in place of `isf_FragNormCoord`,
/// so the bridge is right exactly when the two render the same picture.
/// Asserted as an equivalence rather than against fixed corners, so it says
/// what it means whatever the pipeline's Y convention turns out to be.
#[test]
fn h_material_entry_point_matches_isf_norm_coord() {
    let Some(gpu) = init_gpu() else { return };
    let engine = engine_at(64, 64);
    let (_e, mut state) = load_effect(&gpu, "material.fs");
    let (_e2, mut ref_state) = load_effect(&gpu, "normcoords.fs");

    let material = render_shader(&gpu, "material.fs", &engine, &mut state, None, 64, 64);
    let reference = render_shader(&gpu, "normcoords.fs", &engine, &mut ref_state, None, 64, 64);

    for (x, y) in [(0, 0), (63, 0), (0, 63), (63, 63), (17, 42)] {
        let (r, g, _, a) = material.rgba(x, y);
        let (rr, rg, _, ra) = reference.rgba(x, y);
        assert_channel(r, rr, &format!("R at ({x},{y})"));
        assert_channel(g, rg, &format!("G at ({x},{y})"));
        assert_channel(a, ra, &format!("alpha at ({x},{y})"));
    }
}

/// (i) A `time_base` generator is integrated per frame by the host: it reads
/// zero on the first frame and has advanced by the second. Nothing in the
/// shader or the parameters supplies it, so a non-zero blue channel can only
/// come from the accumulator.
#[test]
fn i_time_base_generator_advances() {
    let Some(gpu) = init_gpu() else { return };
    let mut engine = engine_at(64, 64);
    let (mut effect, mut state) = load_effect(&gpu, "material.fs");
    let descs = effect.parameters();
    engine.custom_param_bases = descs.iter().map(|d| d.default).collect();
    engine.custom_params = engine.custom_param_bases.clone();
    engine.param_descriptors = Arc::new(descs);
    engine.set_param_base("mat_speed", 1.0);

    let first = render_loaded(&gpu, &mut effect, "material.fs", &engine, &mut state, None, None, (8, 8));
    std::thread::sleep(std::time::Duration::from_millis(120));
    let second = render_loaded(&gpu, &mut effect, "material.fs", &engine, &mut state, None, None, (8, 8));

    let (_, _, b_first, _) = first.rgba(4, 4);
    let (_, _, b_second, _) = second.rgba(4, 4);
    eprintln!("generator blue: first={b_first} second={b_second}");
    assert_channel(b_first, 0, "first frame has no elapsed time");
    assert!(
        b_second > 8,
        "generator did not advance over 120ms: {b_second}"
    );
}

/// (i) PERSISTENT pass targets: a two-pass delay line. Pass 0 copies the input
/// into `buf1`; the final pass shows `buf1`, which by the double-buffer rule is
/// what pass 0 wrote *last* frame. So the output trails the input by one frame.
#[test]
fn i_persistent_pass_delays_one_frame() {
    let Some(gpu) = init_gpu() else { return };
    let engine = engine_at(1, 1);
    let (mut effect, mut state) = load_effect(&gpu, "delayline.fs");

    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Delay Input 1x1"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor::default());

    let mut frame = |rgba: [u8; 4]| {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let input = rustjay_core::EffectInput {
            view: &view,
            sampler: &sampler,
            generation: 0,
            texture: None,
        };
        render_loaded(
            &gpu,
            &mut effect,
            "delayline.fs",
            &engine,
            &mut state,
            Some(input),
            None,
            (1, 1),
        )
        .rgba(0, 0)
    };

    let f1 = frame([255, 0, 0, 255]);
    let f2 = frame([0, 255, 0, 255]);
    let f3 = frame([0, 0, 255, 255]);
    eprintln!("delay line: f1={f1:?} f2={f2:?} f3={f3:?}");
    assert_eq!(f1, (0, 0, 0, 0), "frame 1 has no history yet — buf1 is a cleared texture");
    assert_eq!(f2, (255, 0, 0, 255), "frame 2 must show frame 1's red");
    assert_eq!(f3, (0, 255, 0, 255), "frame 3 must show frame 2's green");
}

/// (j) Delta.fs end to end: four chained persistent buffers. A static input has
/// no motion to extract, so it comes out black; a frame that differs from its
/// history does not.
#[test]
fn j_delta_extracts_motion() {
    let Some(gpu) = init_gpu() else { return };
    let engine = engine_at(1, 1);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders/Delta.fs");
    let mut effect = IsfEffect::from_path(&path).expect("Delta.fs");
    EffectPlugin::init(&mut effect, &gpu.device, &gpu.queue);
    assert!(effect.transpile_error.is_none(), "{:?}", effect.transpile_error);
    let mut state = effect.default_state();

    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Delta Input 1x1"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor::default());

    let mut frame = |rgba: [u8; 4]| {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let input = rustjay_core::EffectInput {
            view: &view,
            sampler: &sampler,
            generation: 0,
            texture: None,
        };
        render_loaded(
            &gpu,
            &mut effect,
            "Delta.fs",
            &engine,
            &mut state,
            Some(input),
            None,
            (1, 1),
        )
        .rgba(0, 0)
    };

    // Fill the whole history with white, so every tap agrees.
    let mut static_out = (0, 0, 0, 0);
    for _ in 0..8 {
        static_out = frame([255, 255, 255, 255]);
    }
    // One black frame: the delayed taps still hold white, so motion appears.
    let moving = frame([0, 0, 0, 255]);
    eprintln!("delta: static={static_out:?} moving={moving:?}");
    assert_eq!(
        (static_out.0, static_out.1, static_out.2),
        (0, 0, 0),
        "a static image has no motion to extract"
    );
    assert!(
        moving.0 as u32 + moving.1 as u32 + moving.2 as u32 > 100,
        "a changed frame must light up at least one channel, got {moving:?}"
    );
}

/// (k) The last pass rendering into a `PERSISTENT` target still reaches the
/// screen, and a `WIDTH`/`HEIGHT` expression sizes that target. Half of white
/// mixed with the buffer each frame converges on white: 128, 191, 223.
#[test]
fn k_last_pass_target_reaches_the_screen() {
    let Some(gpu) = init_gpu() else { return };
    let engine = engine_at(4, 4);
    let (mut effect, mut state) = load_effect(&gpu, "feedbackblit.fs");

    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Feedback Input 4x4"),
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
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
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[255u8; 4 * 4 * 4],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(16),
            rows_per_image: Some(4),
        },
        wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
    );
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor::default());

    let mut frame = || {
        let input = rustjay_core::EffectInput {
            view: &view,
            sampler: &sampler,
            generation: 0,
            texture: None,
        };
        render_loaded(
            &gpu,
            &mut effect,
            "feedbackblit.fs",
            &engine,
            &mut state,
            Some(input),
            None,
            (4, 4),
        )
        .rgba(1, 1)
    };

    let (f1, f2, f3) = (frame(), frame(), frame());
    eprintln!("feedback: f1={f1:?} f2={f2:?} f3={f3:?}");
    assert_channel(f1.0, 128, "frame 1 is half of white");
    assert_channel(f2.0, 191, "frame 2 mixes white with frame 1");
    assert_channel(f3.0, 223, "frame 3 mixes white with frame 2");
}

/// (l) An ISF bundle folder — `Whatever.fs/Whatever.fs.fs`, how a download
/// unzips — loads by pointing at the folder.
#[test]
fn l_a_bundle_folder_loads_its_shader() {
    let dir = std::env::temp_dir().join("rustjay-isf-bundle-test/Bundled.fs");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/imgpassthrough.fs"),
        dir.join("Bundled.fs.fs"),
    )
    .unwrap();
    let effect = IsfEffect::from_path(&dir).expect("bundle folder should load");
    assert_eq!(effect.shader_name, "Bundled.fs");
    std::fs::remove_dir_all(dir.parent().unwrap()).ok();
}

/// (m) A companion `.vs` is the shader's vertex stage: the `marker` varying is
/// written there — from an input and from `RENDERSIZE` — and read back in the
/// fragment. Without it the varying would be zero.
#[test]
fn m_companion_vertex_shader_writes_varyings() {
    let Some(gpu) = init_gpu() else { return };
    let engine = engine_at(64, 256);
    let (_e, mut state) = load_effect(&gpu, "vsvarying.fs");
    let f = render_shader(&gpu, "vsvarying.fs", &engine, &mut state, None, 64, 256);
    let px = f.rgba(10, 10);
    eprintln!("companion vs: {px:?}");
    assert_channel(px.0, 128, "red is the `scale` input (0.5)");
    assert_channel(px.1, 128, "green is 128/RENDERSIZE.y (256)");
}

/// (n) The companion vertex stage must not flip the image. Same corners as (d),
/// but sampled through a coordinate the `.vs` computed — naga's SPIR-V frontend
/// negates `gl_Position.y` by default, which renders the shader upside down.
#[test]
fn n_companion_vertex_shader_keeps_orientation() {
    let Some(gpu) = init_gpu() else { return };
    let (_tex, view, sampler) = input_texture_2x2(&gpu);
    let engine = engine_at(2, 2);
    let (_e, mut state) = load_effect(&gpu, "vspassthrough.fs");
    let input = rustjay_core::EffectInput {
        view: &view,
        sampler: &sampler,
        generation: 0,
        texture: None,
    };
    let f = render_shader(
        &gpu,
        "vspassthrough.fs",
        &engine,
        &mut state,
        Some(input),
        2,
        2,
    );
    assert_eq!(f.rgba(0, 0), (255, 0, 0, 255), "top-left must be red");
    assert_eq!(f.rgba(1, 0), (0, 255, 0, 255), "top-right must be green");
    assert_eq!(f.rgba(0, 1), (0, 0, 255, 255), "bottom-left must be blue");
    assert_eq!(f.rgba(1, 1), (255, 255, 255, 255), "bottom-right must be white");
}

/// A 2×2 texture of one flat colour.
fn solid_texture(
    gpu: &Gpu,
    label: &str,
    rgba: [u8; 4],
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
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
    let texels: Vec<u8> = rgba.iter().copied().cycle().take(4 * 4).collect();
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &texels,
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

/// (m) Two-input binding: the *second* declared image input must sample the
/// host's second texture.
///
/// Without this the engine bound every image input to the primary texture, so a
/// transition mixed a frame with itself and `progress` did nothing visible.
/// Red as `startImage`, blue as `endImage`: progress 0 is pure red, 1 is pure
/// blue, and 0.5 is the midpoint. Asserting all three is what separates "both
/// inputs bind correctly" from "something differs".
#[test]
fn m_second_image_input_binds_to_input_b() {
    let Some(gpu) = init_gpu() else { return };
    let (_a, view_a, samp_a) = solid_texture(&gpu, "Deck A (red)", [255, 0, 0, 255]);
    let (_b, view_b, samp_b) = solid_texture(&gpu, "Deck B (blue)", [0, 0, 255, 255]);

    let (effect, _) = load_effect(&gpu, "twoinput.fs");
    let descs = effect.parameters();
    let mut engine = engine_at(16, 16);
    engine.custom_param_bases = descs.iter().map(|d| d.default).collect();
    engine.custom_params = engine.custom_param_bases.clone();
    engine.param_descriptors = Arc::new(descs);

    for (progress, expect, what) in [
        (0.0f32, (255u8, 0u8, 0u8), "progress 0 → startImage"),
        (1.0, (0, 0, 255), "progress 1 → endImage"),
        (0.5, (128, 0, 128), "progress 0.5 → midpoint"),
    ] {
        engine.set_param_base("progress", progress);
        let (mut fx, mut state) = load_effect(&gpu, "twoinput.fs");
        let frame = render_loaded(
            &gpu,
            &mut fx,
            "twoinput.fs",
            &engine,
            &mut state,
            Some(rustjay_core::EffectInput {
                view: &view_a,
                sampler: &samp_a,
                generation: 0,
                texture: None,
            }),
            Some(rustjay_core::EffectInput {
                view: &view_b,
                sampler: &samp_b,
                generation: 0,
                texture: None,
            }),
            (16, 16),
        );
        let (r, g, b, _) = frame.rgba(8, 8);
        eprintln!("{what}: ({r}, {g}, {b})");
        assert_channel(r, expect.0, &format!("{what} (R)"));
        assert_channel(g, expect.1, &format!("{what} (G)"));
        assert_channel(b, expect.2, &format!("{what} (B)"));
    }
}
