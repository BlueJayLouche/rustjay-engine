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

## 4. Multi-pass effects with feedback

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

## 5. Common patterns

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
  - `delta` — RGB spatial delay with custom tab
  - `waaaves` — 3-pass feedback pipeline
- Explore the [`EffectPlugin`] trait documentation for lifecycle hooks (`init`, `prepare`, `render`).
- Enable feature flags in `Cargo.toml` for optional I/O protocols:
  ```toml
  [dependencies]
  rustjay-engine = { version = "0.1", features = ["ndi", "webcam"] }
  ```
