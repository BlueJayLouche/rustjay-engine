# Your First Effect

We'll build a simple desaturation effect in about 15 minutes. It reads video from your webcam, converts it to greyscale, and exposes an `intensity` parameter you can control from the UI.

## Project layout

```
my-effect/
├── Cargo.toml
└── src/
    ├── main.rs
    └── shaders/
        └── desaturate.wgsl
```

## The shader

Create `src/shaders/desaturate.wgsl`:

```wgsl
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) texcoord: vec2<f32>,
) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.texcoord = texcoord;
    return out;
}

// ── Bindings ─────────────────────────────────────────────────────────────
// group(0) — video input (always present)
@group(0) @binding(0) var input_tex:     texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;

// group(1) — your uniforms
struct Uniforms { intensity: f32 };
@group(1) @binding(0) var<uniform> u: Uniforms;

// ── Fragment ─────────────────────────────────────────────────────────────
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let col  = textureSample(input_tex, input_sampler, in.texcoord);
    let grey = dot(col.rgb, vec3<f32>(0.299, 0.587, 0.114));
    let out  = mix(col.rgb, vec3<f32>(grey), u.intensity);
    return vec4<f32>(out, col.a);
}
```

A few things to notice:

- `@group(0) @binding(0/1)` — the live video texture and its sampler. These are always provided by the engine; your shader just declares them.
- `@group(1) @binding(0)` — your uniform block. This is where your per-frame data lives.
- The vertex shader is boilerplate — it passes a full-screen quad through unchanged.

## The Rust side

Edit `src/main.rs`:

```rust
use rustjay_engine::prelude::*;

// ── Plugin struct ─────────────────────────────────────────────────────────
// Holds no data itself — state lives in DesaturateState below.
struct DesaturateEffect;

// ── GPU uniforms ──────────────────────────────────────────────────────────
// Must be repr(C) and implement Pod + Zeroable for bytemuck upload.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DesaturateUniforms {
    intensity: f32,
}

// ── App state ─────────────────────────────────────────────────────────────
// Serialisable so the preset system can save/restore it.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct DesaturateState {
    intensity: f32,
}

// ── Plugin implementation ─────────────────────────────────────────────────
impl EffectPlugin for DesaturateEffect {
    type State    = DesaturateState;
    type Uniforms = DesaturateUniforms;

    fn app_name(&self) -> &str { "my-effect" }

    fn shader_source(&self) -> &'static str {
        include_str!("shaders/desaturate.wgsl")
    }

    fn build_uniforms(&self, s: &DesaturateState, _engine: &EngineState) -> DesaturateUniforms {
        DesaturateUniforms { intensity: s.intensity }
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor::float(
                "intensity", "Intensity",
                ParamCategory::Color,
                0.0, 1.0, 0.0, 0.01,
            ),
        ]
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal", log::LevelFilter::Warn)
        .filter_module("naga",     log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .init();

    rustjay_engine::run(DesaturateEffect)
}
```

## Run it

```sh
cargo run --release
```

Two windows appear. Move the **Intensity** slider in the control window and watch the output change.

> **Release mode** matters for real-time work. Debug builds can be 10–20× slower for GPU-bound effects.

## What just happened?

`rustjay_engine::run()` did a lot for you:

1. Opened a wgpu device on your default GPU
2. Loaded your `DesaturateUniforms` uniform block and compiled your WGSL shader
3. Opened the webcam (or showed black if none available)
4. Created the control window with all built-in tabs
5. Added an **Intensity** slider to the built-in parameter list based on your `parameters()` declaration
6. Starts the render loop, calling `build_uniforms()` every frame

Read [The Two Windows](the-two-windows.md) for a tour of what's in the control window, and [The EffectPlugin Trait](../core-concepts/README.md) for a deep dive into the full API.
