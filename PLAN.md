# rustjay-engine — Architecture & Implementation Plan

> **Role**: System architect (Evidence-First)  
> **Date**: 2026-05-13  
> **Status**: Foundational — API not yet stable

---

## Vision

`rustjay-engine` is to VJ applications what Bevy is to games: a high-performance, cross-platform Rust engine that handles the infrastructure so the artist focuses on the effect.

Every existing rustjay app (`rustjay-template`, `rustjay-delta`, `rustjay-waaaves`, `rustjay-mapper`) duplicates the same ~12 modules — audio, MIDI, OSC, inputs, outputs, presets, GUI scaffolding, event loop. Only the **shader pipeline** and a small set of **app-specific parameters** differ. The engine eliminates that duplication.

An app built on `rustjay-engine` will consist of three things only:
1. A WGSL shader
2. A Uniforms struct (GPU-side parameters)
3. One or more ImGui tabs to expose those parameters

---

## Design Principles

- **Trait-based, not macro-based.** No proc-macros at this stage. The plugin system is plain Rust traits — easy to understand, easy to debug.
- **Generic over app state.** `Engine<S>` where `S` is the app author's state struct. The engine never reaches into `S` directly; it passes it to callbacks.
- **Engine owns the plumbing, app owns the effect.** Audio, MIDI, OSC, presets, I/O, the event loop — all engine. Shaders, uniforms, custom GUI tabs — all app.
- **Full GUI customisability.** Engine renders generic tabs by default. Apps add custom tabs and can replace any built-in tab if they need full control.
- **Feature-flag all optional I/O.** No app should pay compile-time or binary-size cost for NDI when they only use webcam input.
- **Evolve, don't over-engineer.** Boundaries are locked in per phase. Don't design phase 3 in phase 1.

---

## Workspace Layout

```
rustjay-engine/
├── Cargo.toml                  # workspace root
├── PLAN.md
├── crates/
│   ├── rustjay-core/           # shared types, state, LFO, vertex
│   ├── rustjay-audio/          # FFT, beat detection, audio routing
│   ├── rustjay-io/             # all video inputs + outputs (platform-gated)
│   ├── rustjay-control/        # MIDI, OSC, web remote
│   ├── rustjay-presets/        # preset save/load/apply
│   ├── rustjay-gui/            # ImGui scaffolding, generic tabs, GuiTab trait
│   ├── rustjay-render/         # wgpu engine, EffectPlugin trait, pipeline
│   └── rustjay-engine/         # facade: re-exports + RustjayApp builder
└── examples/
    ├── template/               # Phase 1 — HSB color (port of rustjay-template)
    ├── delta/                  # Phase 2 — RGB delay / motion extraction
    └── waaaves/                # Phase 3 — multi-block feedback pipeline
```

### Dependency graph (crates)

```
rustjay-engine (facade)
├── rustjay-render
│   ├── rustjay-core
│   └── rustjay-io
│       └── rustjay-core
├── rustjay-gui
│   ├── rustjay-core
│   ├── rustjay-audio
│   │   └── rustjay-core
│   └── rustjay-control
│       └── rustjay-core
└── rustjay-presets
    └── rustjay-core
```

No cycles. `rustjay-core` depends on nothing in this workspace.

---

## Sub-Crate Responsibilities

### `rustjay-core`

The shared vocabulary. Everything else depends on this; it depends on nothing internal.

- `EngineState` — the state the engine manages (replaces `SharedState` in the template). Contains: `InputState`, `AudioState`, `LfoState`, `HsbParams`, `ResolutionState`, `PerformanceMetrics`, output states, command channels.
- `LfoState` + LFO tick logic (3 banks, 5 waveforms, tempo sync)
- `Vertex` type
- `InputType`, `GuiTab` enum, `BuiltinTab` enum
- Common error types (`RustjayError`)
- Serde-derived types for persistence

### `rustjay-audio`

- `AudioAnalyzer` — lock-free FFT via `realfft`, beat detection, tap tempo
- `AudioRoutingState` — FFT-band → parameter routing matrix
- `AudioCommand` enum
- Feature: always compiled (no optional flag — audio is core to VJ)

### `rustjay-io`

All video sources and sinks. Each is feature-gated. Platform-specific deps live in `[target.*.dependencies]`.

| Feature | Platforms | Crate deps |
|---------|-----------|------------|
| `webcam` | macOS, Windows, Linux | `nokhwa` |
| `ndi` | all | `grafton-ndi` |
| `syphon` | macOS (auto) | `syphon-core`, `syphon-wgpu` |
| `spout` | Windows (auto) | `windows` |
| `v4l2` | Linux (auto) | `v4l` |

Exports: `InputManager`, `OutputManager`, `InputCommand`, `OutputCommand`

### `rustjay-control`

- `MidiManager` — device hot-swap, CC learn system, feature `midi`
- `OscServer` — UDP listener, auto-address generation, feature `osc`
- `WebServer` — axum WebSocket remote, feature `web`
- `MidiCommand`, `OscCommand`, `WebCommand` enums

### `rustjay-presets`

- `PresetBank` — named presets, 8 quick slots, save/load JSON
- `PresetCommand` enum
- Serialises `EngineState` fields + a serialisable snapshot of app state (via trait)

### `rustjay-gui`

- `ImGuiRenderer` — imgui-wgpu integration
- `ControlGui` — dual-window control window, tab bar
- Built-in tabs: **Input**, **Audio**, **Output**, **Presets**, **MIDI**, **OSC**, **Web**, **Settings** — rendered by the engine automatically
- `GuiTab<S>` trait — the extension point for app-specific tabs (see below)

### `rustjay-render`

- `WgpuEngine` — device/queue/surface lifecycle, render target, blit pipeline
- `EffectPlugin` trait — the core abstraction (see below)
- `RenderPipeline` — takes an `EffectPlugin`, manages bind groups and uniform buffers
- `InputTexture`, `Texture` helpers

### `rustjay-engine` (facade)

- Re-exports the public API of all sub-crates under one roof
- `RustjayApp<P>` builder — the entry point for app authors
- `App` struct + winit `ApplicationHandler` impl (the event loop)
- `ConfigManager` — settings persistence

---

## Key Traits

### `EffectPlugin`

```rust
// rustjay-render
pub trait EffectPlugin: Send + Sync + 'static {
    /// App-specific state (parameters, extra textures, etc.)
    type State: Default + Send + Sync + serde::Serialize
        + serde::de::DeserializeOwned + 'static;

    /// GPU uniform type — must be Pod + Zeroable for bytemuck upload
    type Uniforms: bytemuck::Pod + bytemuck::Zeroable;

    /// WGSL source for the main effect shader
    fn shader_source(&self) -> &'static str;

    /// Build uniforms from the current app + engine state, called every frame
    fn build_uniforms(
        &self,
        app_state: &Self::State,
        engine: &EngineState,
    ) -> Self::Uniforms;

    /// Custom GUI tabs to add to (or replace) the built-in tab bar.
    /// Default: empty — only engine tabs are shown.
    fn gui_tabs(&self) -> Vec<Box<dyn GuiTab<State = Self::State>>> {
        vec![]
    }
}
```

### `GuiTab<S>`

```rust
// rustjay-gui
pub trait GuiTab: Send + Sync {
    type State: 'static;

    fn name(&self) -> &str;

    /// Draw this tab's contents. Called every frame while the tab is active.
    fn draw(
        &mut self,
        ui: &imgui::Ui,
        app_state: &mut Self::State,
        engine: &mut EngineState,
    );

    /// If Some, this tab replaces the named built-in tab instead of appending.
    fn replaces(&self) -> Option<BuiltinTab> {
        None
    }
}
```

Full customisability: if an app returns a tab with `replaces() = Some(BuiltinTab::Audio)`, the engine renders the custom tab instead of the built-in Audio tab. Returning tabs for *all* built-ins gives a completely bespoke GUI with no engine chrome.

### `RustjayApp<P>` (builder)

```rust
// rustjay-engine
impl<P: EffectPlugin> RustjayApp<P> {
    pub fn new(plugin: P) -> Self;
    pub fn with_initial_state(mut self, state: P::State) -> Self;
    pub fn run(self) -> anyhow::Result<()>;
}
```

App authors never touch winit, wgpu, or the event loop directly.

---

## What a Complete App Looks Like

```rust
// examples/template/src/main.rs
use rustjay_engine::prelude::*;

struct HsbEffect;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct HsbUniforms {
    values: [f32; 4], // hue_shift, saturation, brightness, _pad
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct HsbState {
    hue_shift:  f32,
    saturation: f32,
    brightness: f32,
    enabled:    bool,
}

impl EffectPlugin for HsbEffect {
    type State    = HsbState;
    type Uniforms = HsbUniforms;

    fn shader_source(&self) -> &'static str {
        include_str!("shaders/hsb.wgsl")
    }

    fn build_uniforms(&self, s: &HsbState, engine: &EngineState) -> HsbUniforms {
        // Composite: base + LFO modulation + audio routing
        let hue = s.hue_shift + engine.lfo.modulate(LfoTarget::Hue);
        HsbUniforms { values: [hue, s.saturation, s.brightness, 0.0] }
    }

    fn gui_tabs(&self) -> Vec<Box<dyn GuiTab<State = HsbState>>> {
        vec![Box::new(ColorTab)]
    }
}

struct ColorTab;
impl GuiTab for ColorTab {
    type State = HsbState;
    fn name(&self) -> &str { "Color" }
    fn draw(&mut self, ui: &imgui::Ui, s: &mut HsbState, _engine: &mut EngineState) {
        ui.slider("Hue Shift", -180.0_f32..=180.0, &mut s.hue_shift);
        ui.slider("Saturation", 0.0_f32..=2.0, &mut s.saturation);
        ui.slider("Brightness", 0.0_f32..=2.0, &mut s.brightness);
    }
}

fn main() -> anyhow::Result<()> {
    RustjayApp::new(HsbEffect)
        .with_initial_state(HsbState {
            saturation: 1.0,
            brightness: 1.0,
            ..Default::default()
        })
        .run()
}
```

---

## Phased Roadmap

---

### Phase 1 — Engine Core + Template Example

**Goal**: `rustjay-engine` is a real Cargo workspace. `examples/template` builds, runs, and is feature-identical to the standalone `rustjay-template` repo.

**Why this proves the engine**: template covers every subsystem — all I/O paths, audio, MIDI, OSC, presets, LFO, dual-window GUI. If the template runs through the engine, every subsystem is wired.

#### Steps

1. **Workspace scaffold**
   - Root `Cargo.toml` declaring all 8 crates as workspace members
   - Stub `lib.rs` in each crate (empty but compiling)
   - Confirm `cargo build` succeeds on the empty workspace

2. **`rustjay-core`**
   - Migrate `core/state.rs`, `core/lfo.rs`, `core/vertex.rs`, `core/mod.rs` from template
   - Rename `SharedState` → `EngineState` (prepares for the generic split)
   - Add `BuiltinTab` enum

3. **`rustjay-audio`**
   - Migrate `audio/fft.rs`, `audio/device.rs`, `audio/routing.rs`, `audio/mod.rs`
   - Keep `AudioCommand` enum here; re-export from `rustjay-core`

4. **`rustjay-io`**
   - Migrate `input/`, `output/`, `v4l2_devices.rs`, `ndi_runtime.rs`
   - All platform-conditional code stays; feature flags preserved
   - Migrate `build.rs` (Syphon/NDI path detection)

5. **`rustjay-control`**
   - Migrate `midi/`, `osc/`, `web/`

6. **`rustjay-presets`**
   - Migrate `presets/`
   - Design `AppStateSnapshot` trait: a simple `fn snapshot(&self) -> serde_json::Value` / `fn restore(&mut self, v: serde_json::Value)` that `EffectPlugin::State` must satisfy via `Serialize`/`Deserialize` (already required)

7. **`rustjay-gui`**
   - Migrate `gui/` (renderer + all tabs)
   - Define `GuiTab<S>` trait
   - Refactor `ControlGui` to accept a `Vec<Box<dyn GuiTab<State=S>>>` alongside the built-in tabs
   - Built-in tabs render first unless replaced via `replaces()`

8. **`rustjay-render`**
   - Migrate `engine/` (renderer, pipeline, blit, texture, uniforms)
   - Define `EffectPlugin` trait
   - `RenderPipeline<P>` wraps `WgpuEngine` and calls `plugin.build_uniforms()` + `plugin.shader_source()` at init

9. **`rustjay-engine` (facade)**
   - Thin `lib.rs`: `pub use rustjay_render::*; pub use rustjay_gui::*;` etc.
   - `prelude` module
   - Migrate `app/` (event loop, commands, update, events) here
   - Make `App<P: EffectPlugin>` generic — holds `P`, `P::State`, and `EngineState`
   - `RustjayApp<P>` builder
   - `ConfigManager` / `config/`

10. **`examples/template`**
    - Implement `HsbEffect` + `HsbState` + `ColorTab` (as sketched above)
    - Copy `shaders/hsb.wgsl`
    - Minimal `Cargo.toml`: `rustjay-engine = { path = "../../crates/rustjay-engine", features = ["ndi", "webcam"] }`

11. **Parity verification checklist**
    - [ ] Webcam input works
    - [ ] NDI input works
    - [ ] Syphon input works (macOS)
    - [ ] Spout input works (Windows)
    - [ ] V4L2 input works (Linux)
    - [ ] NDI output works
    - [ ] Syphon output works (macOS)
    - [ ] Spout output works (Windows)
    - [ ] V4L2 loopback output works (Linux)
    - [ ] Audio FFT + beat detection works
    - [ ] Audio routing matrix works
    - [ ] MIDI CC learn + mapping works
    - [ ] OSC server works
    - [ ] Web remote works
    - [ ] LFO modulation affects shader uniforms
    - [ ] Presets save/load/quick-slots work
    - [ ] Settings auto-save on exit
    - [ ] Fullscreen toggle works
    - [ ] Dual-window layout correct
    - [ ] HSB Color tab renders and controls the shader

**Phase 1 exit criteria**: `cargo run -p template --release` on macOS, Windows, and Linux, with the above checklist complete.

---

### Phase 2 — Delta Example (Proving Generality)

**Goal**: a second example with a *different* shader and *different* app state runs through the same engine without any changes to the engine crates.

`rustjay-delta` uses a multi-frame RGB delay technique (Posy's motion extraction). Its state is:
- `delay_r`, `delay_g`, `delay_b` (frame offsets per channel)
- A circular frame buffer (ring buffer of textures)
- `mix_amount`

This example will stress-test the engine because it requires:
- Multiple GPU textures managed by the app (not just an input texture)
- A uniform type structurally different from `HsbUniforms`
- A different custom tab ("Motion" tab)

**Steps**

1. Implement `DeltaEffect` as `EffectPlugin` with a ring-buffer texture approach
2. Implement `MotionTab` as `GuiTab`
3. If the ring buffer requires hooks the engine doesn't expose, add them minimally (e.g., `fn on_device_ready(&mut self, device: &wgpu::Device, queue: &wgpu::Queue)` lifecycle method on `EffectPlugin`)
4. Identify and document any friction points in the trait API
5. Refine `EffectPlugin` based on what delta required — this is the API's first real test

**Phase 2 exit criteria**: `cargo run -p delta --release` works. No changes to `rustjay-engine`, `rustjay-render`, or `rustjay-gui` — only `examples/delta` is new.

---

### Phase 3 — Multi-Pass / Feedback Rendering (Waaaves-Style)

**Goal**: support multi-stage pipelines where the output of one pass feeds into the next, and where previous frames feed back into the current frame.

`rustjay-waaaves` has three shader blocks running in sequence with cross-block feedback. This pattern (TouchDesigner-style feedback loops) is fundamental to VJ aesthetics and needs first-class engine support.

**Design sketch** (to be finalised after Phase 2 feedback):

Option A — `MultiPassPlugin` extending `EffectPlugin` with a `passes()` method returning an ordered list of `RenderPass` descriptors.

Option B — A `RenderGraph` type that `EffectPlugin::configure_graph()` populates. Nodes are passes; edges declare which texture one pass's output feeds into as another's input.

Option B aligns better with wgpu's mental model and scales to complex graphs (waaaves's 3-pass feedback, mapper's mesh warping). Evaluate after Phase 2.

**Steps**

1. Design `RenderGraph` API (informed by Phase 2 lessons)
2. Extend `rustjay-render` with multi-pass support
3. Add `PreviousFrameTexture` resource (the feedback mechanism)
4. Implement `WaaavesEffect` with 3 chained passes
5. Port waaaves shader blocks as WGSL
6. Implement `WaaavesTab` (feedback amount, mix controls)

**Phase 3 exit criteria**: `cargo run -p waaaves --release` works. Single-pass apps (template, delta) are unaffected.

---

### Phase 4 — API Stabilisation + Publishing

**Goal**: a stable, documented, publishable crate.

- Audit all public APIs for semver soundness
- `#![deny(missing_docs)]` on all public items
- Write a guide (docs.rs-hosted) covering: getting started, writing an effect, writing a custom tab, multi-pass effects
- Update `rustjay-new` project generator to scaffold against `rustjay-engine`
- Evaluate whether existing standalone apps (`rustjay-template`, `rustjay-delta`) should become thin wrappers over their engine examples
- Publish to crates.io

---

## Open Questions (Deferred to Later Phases)

These are real design questions that don't need answers in Phase 1 but should be resolved before Phase 4.

| Question | When to decide |
|---|---|
| Should `EffectPlugin::State` be required to implement a `PresetSnapshot` trait, or is `Serialize`/`Deserialize` enough? | End of Phase 1 |
| Does `EffectPlugin` need an `on_device_ready()` lifecycle hook for GPU resource init? | Phase 2 |
| `RenderGraph` vs `MultiPassPlugin` for multi-pass effects | Start of Phase 3 |
| Should `rustjay-audio` be optional (feature flag)? Some visual-only apps may not want it | Phase 3 |
| Workspace-level version pinning strategy (one version for all crates vs independent versioning) | Phase 4 |
| `rustjay-engine` on crates.io: one facade crate or individual sub-crates published separately? | Phase 4 |

---

## Migration Path for Existing Apps

After Phase 2 proves the API is stable enough:

1. `rustjay-template` → add `rustjay-engine` as a dep, delete the 12 duplicate modules, keep only the HSB shader + Color tab + `main.rs`
2. `rustjay-delta` → same, keep only the delay shader + Motion tab
3. `rustjay-waaaves` → after Phase 3, keep only the 3 shader blocks + Waaaves tabs
4. `rustjay-mapper` → scope-defined separately (projection mesh warping may require Phase 3+ capabilities)

The goal is that each existing app's unique code shrinks to ~100–200 lines of Rust + its WGSL shader(s).

---

## Non-Goals

- `rustjay-engine` is **not** a general-purpose game engine. It has no scene graph, no entity-component system, no physics. The primitives are: video texture in, shader effect, video texture out.
- No OpenGL or Vulkan backends. wgpu is the only graphics layer.
- No scripting language (Lua, Rhai) in Phase 1–3. WGSL is compiled at build time.
- No audio synthesis. `rustjay-audio` analyses audio; it does not generate it.
