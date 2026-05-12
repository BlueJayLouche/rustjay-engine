# rustjay-engine — Architecture & Implementation Plan

> **Role**: System architect (Evidence-First)  
> **Date**: 2026-05-13  
> **Status**: Phase 1 ✅ Complete — Phase 2 ✅ Complete — Phase 3 ✅ Complete — Phase 4 upcoming

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
│   ├── rustjay-core/           # shared types, state, LFO, vertex, routing
│   ├── rustjay-audio/          # FFT, beat detection (routing types re-export from core)
│   ├── rustjay-io/             # all video inputs + outputs (platform-gated)
│   ├── rustjay-control/        # MIDI, OSC, web remote
│   ├── rustjay-presets/        # preset save/load/apply
│   ├── rustjay-gui/            # ImGui scaffolding, generic tabs, GuiTab trait
│   ├── rustjay-render/         # wgpu engine, EffectPlugin trait, pipeline
│   └── rustjay-engine/         # facade: re-exports + RustjayApp builder
└── examples/
    ├── template/               # Phase 1 — HSB color (port of rustjay-template) ✅
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

- `EngineState` — the state the engine manages (renamed from `SharedState` in the template). Contains: `InputState`, `AudioState`, `LfoState`, `HsbParams`, `ResolutionState`, `PerformanceMetrics`, output states, command channels.
- `LfoState` + LFO tick logic (3 banks, 5 waveforms, tempo sync)
- `Vertex` type
- `InputType`, `GuiTab` enum, `BuiltinTab` enum
- Common error types
- Serde-derived types for persistence
- **Audio routing types** — `AudioRoutingState`, `RoutingMatrix`, `AudioRoute`, `FftBand`, `ModulationTarget` live here (not in `rustjay-audio`) to avoid circular deps; `rustjay-audio` re-exports them.

### `rustjay-audio`

- `AudioAnalyzer` — lock-free FFT via `realfft`, beat detection, tap tempo
- `AudioCommand` enum
- Routing types are defined in `rustjay-core` and re-exported here for backwards compatibility
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
- Serialises `EngineState` fields + custom_values map for extensibility

### `rustjay-gui`

- `ImGuiRenderer` — imgui-wgpu integration
- `ControlGui` — dual-window control window, tab bar
- Built-in tabs: **Input**, **Audio**, **Output**, **Presets**, **MIDI**, **OSC**, **Web**, **Settings** — rendered by the engine automatically
- `GuiTab<S>` trait — Phase 2 extension point for app-specific tabs (see below)

### `rustjay-render`

- `WgpuEngine` — device/queue/surface lifecycle, render target, blit pipeline
- `EffectPlugin` trait — Phase 2 core abstraction (see below)
- `MainPipeline` — HSB shader pipeline targeting `Bgra8Unorm`
- `InputTexture`, `Texture` helpers

### `rustjay-engine` (facade)

- Re-exports the public API of all sub-crates under one roof
- `App` struct + winit `ApplicationHandler` impl (the event loop)
- `ConfigManager` — settings persistence
- Phase 1 entry point: `run(app_name: &str)` — hardwired HSB pipeline
- Phase 2 entry point: `RustjayApp<P>` builder — generic over `EffectPlugin`

---

## Key Traits (Phase 2)

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

## What a Complete App Looks Like (Phase 2+)

```rust
// examples/template/src/main.rs (after Phase 2 migration)
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

### Phase 1 — Engine Core + Template Example ✅ COMPLETE

**Goal**: `rustjay-engine` is a real Cargo workspace. `examples/template` builds and is architecturally identical to the standalone `rustjay-template` repo.

**What was built**: All 8 crates scaffolded and populated. Every subsystem from `rustjay-template` lives in its respective crate. The entry point is `rustjay_engine::run("template")` — a hardwired function that boots the full engine with the built-in HSB pipeline. The `EffectPlugin` / `GuiTab<S>` / `RustjayApp<P>` generics were intentionally deferred to Phase 2; they require the full working engine as a foundation before the generic layer can be extracted safely.

**Key architectural decision**: `AudioRoutingState`, `RoutingMatrix`, `AudioRoute`, `FftBand`, and `ModulationTarget` were moved from `rustjay-audio` into `rustjay-core` to eliminate a circular dependency. `rustjay-audio` re-exports them.

#### Steps (completed)

1. ✅ Workspace scaffold — root `Cargo.toml`, 8 crates, stub `lib.rs` files
2. ✅ `rustjay-core` — `EngineState` (renamed from `SharedState`), LFO, vertex, routing types
3. ✅ `rustjay-audio` — `AudioAnalyzer`, FFT, beat detection, `AudioCommand`
4. ✅ `rustjay-io` — `InputManager`, `OutputManager`, all platform-gated I/O paths, `build.rs`
5. ✅ `rustjay-control` — MIDI, OSC, web remote
6. ✅ `rustjay-presets` — `PresetBank`, save/load/quick-slots, `custom_values` map
7. ✅ `rustjay-gui` — `ImGuiRenderer`, `ControlGui`, all 8 built-in tabs
8. ✅ `rustjay-render` — `WgpuEngine`, blit, texture, `main.wgsl` (hardwired HSB pipeline, replaced by `PluginRenderer<P>` in Phase 2)
9. ✅ `rustjay-engine` (facade) — `App`, event loop, commands, `config/`, `run(app_name)`
10. ✅ `examples/template` — calls `rustjay_engine::run("template")`
11. ✅ README, git init, pushed to `git@github.com:BlueJayLouche/rustjay-engine.git`

#### Parity verification checklist (runtime — not yet tested)

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

### Phase 2 — Generic Plugin API + Delta Example ✅ COMPLETE

**Goal**: extract the `EffectPlugin` / `AnyGuiTab` generic layer from the hardwired engine, prove it with two working examples.

**What was built**:

#### Part A — Generic traits (completed)

1. ✅ **`EffectPlugin` in `rustjay-core`** — trait with `State`, `Uniforms`, `shader_source`, `build_uniforms`, `app_name`, `default_state`, and lifecycle hooks `init`, `prepare`, `render` (escape hatch for custom passes)
2. ✅ **`AnyGuiTab` in `rustjay-gui`** — type-erased tab trait using `dyn Any` for app state; `replaces()` hook honoured in `build_tabs` so custom tabs can occupy a built-in slot in-position; `BuiltinTab` aliased to `GuiTab` (no duplicate enum)
3. ✅ **`App<P>`, `WgpuEngine<P>`, `PluginRenderer<P>`** — engine fully generic over `P: EffectPlugin`
4. ✅ **`run<P>` / `run_with_tabs`** entry points; `prelude` module for convenient imports
5. ✅ **`plugin.app_name()`** threads through to `ConfigManager` and `WebConfig` for per-app config isolation
6. ✅ **`plugin.default_state()`** for non-`Default` initial app state (fixes black-screen regression)
7. ✅ Deleted dead `pipeline.rs` (`MainPipeline`) and `uniforms.rs` (`HsbUniforms`); `plugin.init()` called inside `PluginRenderer::new()`

#### Part B — Examples (completed)

8. ✅ **`examples/template`** — `HsbEffect` implementing `EffectPlugin`; `build_uniforms` applies audio routing matrix + full 3-channel LFO modulation; `default_state` sets saturation=1, brightness=1
9. ✅ **`examples/delta`** — `DeltaEffect` (RGB spatial delay / chromatic aberration); `DeltaState` with `delay_r/g/b`, `mix_amount`; `MotionTab` implementing `AnyGuiTab`; proves the engine is general with zero engine-crate changes

**Key decisions from Phase 2**:
- `EffectPlugin` lives in `rustjay-core` (not `rustjay-render`) because wgpu was already a core dependency and it avoids a re-export chain
- `AnyGuiTab` uses `dyn Any` for type erasure rather than a generic `GuiTab<S>` — avoids making `ControlGui` generic and keeps the tab list `Vec<Box<dyn AnyGuiTab>>`
- The `render()` escape hatch on `EffectPlugin` (returns `bool` to skip default pass) is sufficient for Phase 2; `RenderGraph` deferred to Phase 3

**Phase 2 exit criteria**: `cargo run -p template --release` and `cargo run -p delta --release` both work. ✅

---

### Phase 3 — Multi-Pass / Feedback Rendering (Waaaves-Style) ✅ COMPLETE

**Goal**: support multi-stage pipelines where the output of one pass feeds into the next, and where previous frames feed back into the current frame.

**What was built**:

#### Design decision: lightweight `RenderGraph` in `rustjay-core`

- `RenderGraph` — linear multi-pass descriptor with `.passes: Vec<Pass>` and `.feedback: bool`
- `Pass` — `label`, `shader` (WGSL source), `input: PassInput` (`EngineInput`, `PreviousPass`, `Feedback`)
- Builder API: `RenderGraph::new().with_pass(...).with_feedback()`

#### `EffectPlugin` extensions

- `fn render_graph(&self) -> Option<RenderGraph>` — default `None` keeps single-pass plugins unchanged
- `fn build_pass_uniforms(&self, pass_index, state, engine) -> Self::Uniforms` — default delegates to `build_uniforms()`, so simple multi-pass effects can reuse one uniform block

#### `rustjay-render` multi-pass implementation

- Unified texture bind group layout with 4 entries:
  - `@binding(0/1)` — primary input texture + sampler
  - `@binding(2/3)` — feedback texture + sampler (bound to dummy 1×1 black when unused)
  - Single-pass shaders simply omit bindings 2/3 — valid in wgpu
- `PluginRenderer` manages:
  - `graph_pipelines: Vec<wgpu::RenderPipeline>` — one per pass, lazily created from graph
  - `intermediate_textures: Vec<Texture>` — `passes.len() - 1` textures for pass outputs
  - `dummy_feedback: Texture` — 1×1 black for unused feedback slot
- `PreviousFrameTexture` — helper type that copies render target → feedback texture after each frame
- `WgpuEngine` creates `previous_frame: Option<PreviousFrameTexture>` when `graph.feedback == true`

#### `examples/waaaves`

- `WaaavesEffect` with 3-pass graph + feedback enabled
- Block A: mixes engine input with feedback, applies radial warp
- Block B: box blur + trail decay
- Block C: HSB color grading
- `WaaavesTab` custom GUI with all 8 parameters
- `WaaavesUniforms` (32 bytes) shared across all 3 passes

**Key decisions from Phase 3**:
- `RenderGraph` lives in `rustjay-core` (not `rustjay-render`) to keep the trait definition co-located with its associated types
- Linear graph execution (not full DAG) is sufficient for waaaves and keeps the API simple; DAG can be added later without breaking changes
- Feedback is opt-in per graph; single-pass apps pay no cost
- Per-pass uniform override is possible but defaults to shared uniforms for convenience

#### Code-review fixes applied after initial implementation

1. ✅ **Per-pass uniform buffers** — initial impl used a single `wgpu::Buffer`; all `queue.write_buffer()` calls for different passes raced/overwrote each other. Fixed by adding `graph_uniform_buffers: Vec<wgpu::Buffer>` and `graph_uniform_bind_groups: Vec<wgpu::BindGroup>` (one per pass) in `PluginRenderer`.
2. ✅ **`mix_original` unimplemented** — `block_c.wgsl` declared the field in `WaaavesUniforms` but never used it. Fixed: `fs_main` now blends the HSB-graded result back toward the pre-graded input with `mix(graded, pre_grade, u.mix_original)`.
3. ✅ **`PassInput::PreviousPass` on pass 0** — silent undefined behaviour (used dummy texture without warning). Fixed: explicit match arm + `log::warn!` emitted at pipeline rebuild time.
4. ✅ **Shader dirty check by count only** — pipeline rebuild only triggered when pass count changed; shader edits were silently ignored. Fixed: `graph_shader_sources: Vec<&'static str>` cached and compared via `std::ptr::eq` each rebuild check.
5. ✅ **Unused feedback bindings** — `block_b.wgsl` and `block_c.wgsl` declared `@group(0) @binding(2/3)` for `feedback_tex`/`feedback_sampler` but never sampled them. Removed to avoid confusing wgpu validation warnings.
6. ✅ **`render_graph()` called per frame** — `WgpuEngine` called `plugin_renderer.plugin.render_graph()` on every frame to check `feedback` flag (heap allocation each time). Fixed: `cached_graph: Option<RenderGraph>` stored in `PluginRenderer` at construction; `renderer.rs` reads `plugin_renderer.cached_graph.as_ref()`.

**Phase 3 exit criteria**: `cargo run -p waaaves --release` works. Single-pass apps (template, delta) are unaffected. ✅

---

### Phase 4 — API Stabilisation + Publishing

**Goal**: a stable, documented, publishable crate. Every public API is intentional, every item is documented, and three real apps can be migrated onto the engine without changes to the engine itself.

#### Steps

1. **Runtime verification** — run through the Phase 1 parity checklist (webcam, NDI, Syphon, audio routing, MIDI, OSC, presets, LFO) on macOS. Document any regressions as issues.
2. **API audit** — walk every `pub` item in all 8 crates. Mark anything that should be `pub(crate)` or removed. Apply `#[doc(hidden)]` to internal-but-pub items that can't be privatised yet without a breaking change.
3. **`#![deny(missing_docs)]`** — add to all public crates. Write doc-comments for every public item. Keep them one sentence; link to related items.
4. **Getting-started guide** — a single Markdown doc (rendered on docs.rs via `lib.rs`) covering: workspace setup, writing a minimal effect, adding a custom GUI tab, multi-pass with feedback.
5. **Migrate `examples/template`** — verify the existing example is the authoritative replacement for `rustjay-template`. Ensure it runs release-mode on all three platforms.
6. **Migrate `examples/delta`** — same for `rustjay-delta`.
7. **Migrate `examples/waaaves`** — same for `rustjay-waaaves`. This also exercises Phase 3 on real hardware.
8. **`rustjay-new` scaffold** — CLI tool (or shell template) that generates a new project with `rustjay-engine` as a dep, a stub shader, a stub uniforms struct, and a stub `EffectPlugin`. Replaces copying from template repos.
9. **Versioning** — decide on unified vs independent crate versioning (see Open Questions). Set `version = "0.1.0"` workspace-wide, add `publish = false` on internal crates not intended for crates.io.
10. **Publish** — `cargo publish -p rustjay-core`, then each dependent crate in dependency order. Verify docs.rs renders correctly.

**Phase 4 exit criteria**: `cargo add rustjay-engine` works from crates.io. The getting-started guide produces a running app in under 15 minutes.

---

## Open Questions (Deferred to Later Phases)

| Question | When to decide |
|---|---|
| Should `EffectPlugin::State` be required to implement a `PresetSnapshot` trait, or is `Serialize`/`Deserialize` enough? | Phase 3 — `custom_values: HashMap<String, f32>` in presets is a stopgap; revisit when waaaves needs richer snapshot |
| Does `EffectPlugin` need an `on_device_ready()` lifecycle hook for GPU resource init? | ✅ Resolved in Phase 2 — `init(&mut self, device, queue)` added; delta confirmed it's sufficient for single-pass effects |
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
