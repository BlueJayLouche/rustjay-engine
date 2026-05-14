# Getting Started with rustjay-engine

> Build a real-time video effect in under 15 minutes.

## What you need

- **Rust** 1.80+ with `cargo`
- A working webcam or video capture device (optional — the engine shows a black input if none is available)
- macOS, Windows, or Linux

## 1. Create a new project

```bash
cargo new my-effect
cd my-effect
```

Add the engine to `Cargo.toml`:

```toml
[dependencies]
rustjay-engine = "0.1"
bytemuck = { version = "1.21", features = ["derive"] }
serde = { version = "1.0", features = ["derive"] }
anyhow = "1.0"
```

## 2. Write your first effect

Create `src/shaders/my_effect.wgsl`:

```wgsl
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
};

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.texcoord = texcoord;
    return out;
}

struct MyUniforms {
    intensity: f32,
};

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;
@group(1) @binding(0) var<uniform> u: MyUniforms;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let col = textureSample(input_tex, input_sampler, in.texcoord);
    let gray = dot(col.rgb, vec3<f32>(0.299, 0.587, 0.114));
    let mixed = mix(col.rgb, vec3<f32>(gray), u.intensity);
    return vec4<f32>(mixed, col.a);
}
```

Edit `src/main.rs`:

```rust,ignore
use rustjay_engine::prelude::*;

struct MyEffect;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct MyUniforms {
    intensity: f32,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct MyState {
    intensity: f32,
}

impl EffectPlugin for MyEffect {
    type State = MyState;
    type Uniforms = MyUniforms;

    fn app_name(&self) -> &str { "my-effect" }

    fn shader_source(&self) -> &'static str {
        include_str!("shaders/my_effect.wgsl")
    }

    fn build_uniforms(&self, s: &MyState, _engine: &EngineState) -> MyUniforms {
        MyUniforms { intensity: s.intensity }
    }
}

fn main() -> anyhow::Result<()> {
    rustjay_engine::run(MyEffect)
}
```

Run it:

```bash
cargo run --release
```

You should see two windows: a control window with tabs (Input, Audio, Output, etc.) and a fullscreen output window.

## 3. Add a custom GUI tab

Implement the [`AnyGuiTab`] trait to add sliders, buttons, or any ImGui widgets:

```rust,ignore
struct MyTab;

impl AnyGuiTab for MyTab {
    fn name(&self) -> &str { "My Effect" }

    fn draw(
        &mut self,
        ui: &imgui::Ui,
        app_state: &mut dyn std::any::Any,
        _engine: &mut EngineState,
    ) {
        let state = app_state
            .downcast_mut::<MyState>()
            .expect("MyTab expects MyState");

        ui.slider_config("Intensity", 0.0, 1.0)
            .build(&mut state.intensity);
    }
}
```

Then pass it to [`run_with_tabs`]:

```rust,ignore
fn main() -> anyhow::Result<()> {
    rustjay_engine::run_with_tabs(MyEffect, vec![Box::new(MyTab)])
}
```

Your tab appears in the control window's tab bar. If you want it to *replace* a built-in tab (e.g. replace the Color tab), implement [`AnyGuiTab::replaces`]:

```rust,ignore
impl AnyGuiTab for MyTab {
    // ...
    fn replaces(&self) -> Option<BuiltinTab> {
        Some(BuiltinTab::Color)
    }
}
```

## 4. Frame-history effects with a custom render pipeline

For effects that need to read from multiple *previous frames* (temporal delay, motion extraction, echo trails), override `render()` and manage your own GPU pipeline and ring buffer.

```rust,ignore
use rustjay_engine::prelude::*;

struct MyEffect {
    pipeline:   Option<wgpu::RenderPipeline>,
    tex_bgl:    Option<wgpu::BindGroupLayout>,
    history:    Vec<Texture>,       // ring buffer of past frames
    write_idx:  usize,
}

impl EffectPlugin for MyEffect {
    type State = MyState;
    type Uniforms = MyUniforms;

    /// Return a stub shader that matches the engine's default binding layout
    /// (group 0: texture, sampler, texture, sampler — group 1: uniform).
    /// The engine compiles this for its default pipeline, but since render()
    /// returns true below, that pipeline is never used.
    fn shader_source(&self) -> &'static str {
        include_str!("shaders/stub.wgsl")
    }

    /// Build the real pipeline with your own bind group layout.
    fn init(&mut self, device: &wgpu::Device, _queue: &wgpu::Queue) {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("My Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/real.wgsl").into()),
        });
        // ... create bind group layouts, pipeline, history textures ...
        self.pipeline = Some(/* ... */);
    }

    /// Custom render — copy the live input into the ring buffer, then draw
    /// using delayed frames for each channel.  Return true to tell the engine
    /// to skip its own render pass.
    fn render(
        &mut self,
        encoder:             &mut wgpu::CommandEncoder,
        device:              &wgpu::Device,
        queue:               &wgpu::Queue,
        input_view:          Option<&wgpu::TextureView>,
        input_sampler:       Option<&wgpu::Sampler>,
        render_target_view:  &wgpu::TextureView,
        app_state:           &mut Self::State,
        engine_state:        &EngineState,
        _vertex_buffer:      &wgpu::Buffer,
        input_texture:       Option<&wgpu::Texture>,  // raw texture — use for GPU copies
    ) -> bool {
        // Push current frame into ring buffer
        if let Some(src) = input_texture {
            let dest = &self.history[self.write_idx];
            encoder.copy_texture_to_texture(/* src → dest */);
            self.write_idx = (self.write_idx + 1) % self.history.len();
        }

        // Build bind group from delayed history frames, run render pass ...

        true  // skip the engine's default render pass
    }
}
```

Key rules:
- **`shader_source()` still required** — the engine compiles it for its default pipeline even if you never use that pipeline. Make it a minimal stub whose bindings match the engine's standard layout (`[Texture, Sampler, Texture, Sampler]` at group 0, uniform buffer at group 1).
- **Your real shader** lives in a separate file compiled inside `init()`, with whatever binding layout you need.
- **`input_texture`** gives you the raw `wgpu::Texture` so you can `copy_texture_to_texture` without an intermediate CPU readback.
- Returning `true` from `render()` tells the engine to skip its default draw — you own the render target.

See `examples/delta` for a complete working implementation: 8-frame RGB ring buffer, 8 blend modes, per-channel gain, threshold, and trail fade.

## 5. Multi-pass effects with feedback

For effects that need multiple shader stages or frame feedback, return a [`RenderGraph`]:

```rust,ignore
impl EffectPlugin for MyEffect {
    // ... single-pass shader still required as fallback ...

    fn render_graph(&self) -> Option<RenderGraph> {
        Some(
            RenderGraph::new()
                .with_pass(Pass {
                    label: "Blur",
                    shader: include_str!("shaders/blur.wgsl"),
                    input: PassInput::EngineInput,
                })
                .with_pass(Pass {
                    label: "Feedback Mix",
                    shader: include_str!("shaders/mix.wgsl"),
                    input: PassInput::PreviousPass,
                })
                .with_feedback(), // enables previous-frame texture at @binding(2/3)
        )
    }
}
```

Each pass can read from:
- `PassInput::EngineInput` — the live video source
- `PassInput::PreviousPass` — the output of the previous pass in the graph
- `PassInput::Feedback` — the previous frame's final output

The last pass always writes to the render target.

## 6. Common patterns

### Reading audio data

```rust,ignore
fn build_uniforms(&self, s: &MyState, engine: &EngineState) -> MyUniforms {
    let volume = engine.audio.volume;
    let bass = engine.audio.fft[0];
    // ...
}
```

### LFO modulation

```rust,ignore
let (hue_mod, sat_mod, bright_mod) = engine.lfo.bank.get_hsb_modulations();
```

### Preset save/load

Presets are handled automatically by the engine. The built-in Presets tab lets users save snapshots of the entire [`EngineState`] plus your app's [`EffectPlugin::State`].

## Next steps

- Browse the [`examples/`] directory in the repo for complete working apps:
  - `template` — HSB colour adjustment
  - `delta` — RGB delay / motion extraction with custom render pipeline and frame-history ring buffer
  - `waaaves` — 3-pass feedback pipeline
- Explore the [`EffectPlugin`] trait documentation for lifecycle hooks (`init`, `prepare`, `render`).
- Enable feature flags in `Cargo.toml` for optional I/O protocols:
  ```toml
  [dependencies]
  rustjay-engine = { version = "0.1", features = ["ndi", "webcam"] }
  ```

### Tempo sync (Ableton Link + ProDJ Link)

The engine can lock to external tempo sources. Enable the features you need:

```toml
[dependencies]
rustjay-engine = { version = "0.1", features = ["link", "prodj"] }
```

In your shader code, use [`EngineState::effective_bpm`] and [`EngineState::effective_beat_phase`]
instead of `engine.audio.bpm` / `engine.audio.beat_phase`:

```rust,ignore
fn build_uniforms(&self, s: &MyState, engine: &EngineState) -> MyUniforms {
    let bpm = engine.effective_bpm();
    let phase = engine.effective_beat_phase();
    // ...
}
```

This automatically follows Ableton Link when peers are present, falls back to
ProDJ Link when Link is unavailable, and finally falls back to audio analysis.
No plugin changes are required beyond using `effective_bpm()` — the built-in
**Sync** tab lets users enable/disable each source at runtime.
