# The EffectPlugin Trait

`EffectPlugin` is the central abstraction of rustjay-engine. It's a trait you implement once per app, and the engine calls its methods at the right times during setup and the render loop.

```rust
pub trait EffectPlugin: Send + Sync + 'static {
    type State:    Default + Send + Sync + Serialize + DeserializeOwned + 'static;
    type Uniforms: bytemuck::Pod + bytemuck::Zeroable;

    // Required
    fn shader_source(&self)                                     -> &'static str;
    fn build_uniforms(&self, state: &Self::State, engine: &EngineState) -> Self::Uniforms;

    // Common optional overrides
    fn app_name(&self)      -> &str                            { "rustjay" }
    fn default_state(&self) -> Self::State                     { Default::default() }
    fn parameters(&self)    -> Vec<ParameterDescriptor>        { vec![] }
    fn hidden_tabs(&self)   -> Vec<GuiTab>                     { vec![] }

    // Lifecycle hooks
    fn init(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {}
    fn prepare(&mut self, state: &mut Self::State, engine: &EngineState,
               device: &wgpu::Device, queue: &wgpu::Queue) {}

    // Rendering overrides
    fn render_graph(&self)           -> Option<RenderGraph>   { None }
    fn mesh_descriptor(&self, state: &Self::State) -> Option<MeshDescriptor> { None }
    fn vertex_reads_texture(&self)   -> bool                  { false }
    fn compute_shader(&self)         -> Option<&'static str>  { None }
    fn render(&mut self, ...) -> bool                         { false }
}
```

## Associated types

### `State`

Your app's mutable runtime state. The engine owns one instance of this, passes `&State` to `build_uniforms()` every frame, and passes `&mut State` to `prepare()` and to your custom GUI tab's `draw()` method.

Requirements:
- `Default` — the engine creates the initial state with `default_state()` (which calls `Default::default()` unless you override it)
- `Serialize + DeserializeOwned` — the preset system serialises this to JSON when saving and restores it when loading

A typical state struct:

```rust
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct MyState {
    intensity: f32,
    hue_shift: f32,
    enabled:   bool,
}
```

### `Uniforms`

The GPU-side data block uploaded to `@group(1) @binding(0)` every frame. Must be:
- `#[repr(C)]` — stable field layout for bytemuck
- `bytemuck::Pod + bytemuck::Zeroable` — safe transmute to bytes

```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct MyUniforms {
    intensity: f32,
    hue_shift: f32,
    _pad:      [f32; 2],   // pad to 16-byte alignment
}
```

> **Alignment:** wgpu requires uniform buffers to be 16-byte aligned. If your struct's size isn't a multiple of 16 bytes, add padding fields.

## Required methods

### `shader_source()`

Returns the WGSL source for your fragment shader. Use `include_str!` to embed a file at compile time:

```rust
fn shader_source(&self) -> &'static str {
    include_str!("shaders/my_effect.wgsl")
}
```

### `build_uniforms()`

Called every frame. Reads from your `State` and `EngineState` to produce the `Uniforms` value that gets uploaded to the GPU:

```rust
fn build_uniforms(&self, s: &MyState, engine: &EngineState) -> MyUniforms {
    MyUniforms {
        intensity: engine.get_param("intensity").unwrap_or(s.intensity),
        hue_shift: s.hue_shift,
        _pad:      [0.0; 2],
    }
}
```

Call `engine.get_param(id)` to read a parameter with LFO and audio modulations already applied. See [EngineState](engine-state.md) for the full API.

## Lifecycle hooks

### `init(device, queue)`

Called once after the wgpu device is ready. Use this to create extra textures, bind groups, or pipelines that the default single-pass setup can't express.

```rust
fn init(&mut self, device: &wgpu::Device, _queue: &wgpu::Queue) {
    let texture = device.create_texture(&wgpu::TextureDescriptor { /* ... */ });
    self.extra_tex = Some(texture);
}
```

### `prepare(state, engine, device, queue)`

Called every frame, *before* the render pass. Use this for per-frame GPU resource updates — writing to a texture, updating a compute buffer — that aren't handled by the uniform upload.

## Declaring parameters

```rust
fn parameters(&self) -> Vec<ParameterDescriptor> {
    vec![
        ParameterDescriptor::float(
            "intensity", "Intensity",      // id, display name
            ParamCategory::Color,          // tab grouping
            0.0, 1.0, 0.5, 0.01,          // min, max, default, step
        ),
        ParameterDescriptor::int(
            "blend_mode", "Blend Mode",
            ParamCategory::Motion,
            0, 7, 0, 1,
        ),
    ]
}
```

Declared parameters:
- Appear as sliders in the built-in control UI
- Can be targeted by LFO banks
- Can be mapped to MIDI CC via learn mode
- Are addressable as OSC messages at `/rustjay/<id>`
- Receive audio-reactive modulation from the routing matrix

Read them back in `build_uniforms()` via `engine.get_param(id)`, which returns the base value plus all active modulations.

## Hiding built-in tabs

If your effect doesn't use colour parameters, you can hide the Color tab to keep the UI clean:

```rust
fn hidden_tabs(&self) -> Vec<GuiTab> {
    vec![GuiTab::Color]
}
```

Available tabs: `GuiTab::Input`, `GuiTab::Audio`, `GuiTab::Lfo`, `GuiTab::Midi`, `GuiTab::Osc`, `GuiTab::Output`, `GuiTab::Presets`, `GuiTab::Color`, `GuiTab::Sync`.

## Entry points

Two engine entry points are available:

```rust
// Simple — no custom tabs
rustjay_engine::run(MyEffect)

// With custom control-window tabs
rustjay_engine::run_with_tabs(MyEffect, vec![Box::new(MyTab)])
```

See [Custom Tabs](../building-ui/custom-tabs.md) for how to implement `AnyGuiTab`.
