# Phase B Roadmap — Extracting Varda's Value into rustjay-engine

**Date:** 2026-06-03
**Author:** Architect role (Evidence-First)
**Scope:** `rustjay-engine` workspace
**Companion docs:** `AUDIT_ROADMAP.md` (security/perf), `WAAAVES_PORT.md` (effect port)

This document is the canonical plan for moving Varda's differentiating
capabilities into rustjay-engine as **optional, feature-gated crates**, without
regressing the single-plugin app model that `delta` / `flux` / `sputnik` /
`waaaves` depend on.

The five capabilities being extracted (from the cross-project review):

| What | Why | Effort |
|---|---|---|
| `rustjay-mixer` — multi-channel compositor | Varda's mixer, as an effect that consumes N inputs | High |
| `rustjay-isf` — GLSL/ISF hot-reload layer | shaderc + naga transpile GLSL → WGSL; Varda's shader library works in-engine | Low–Medium ¹ |
| `rustjay-projection` — dome, warp, edge-blend | Projection mapping as an optional output post-processor | High |
| `rustjay-api` — REST/OpenAPI | Varda's axum routes, extracted as optional; the existing web UI becomes the default | Medium |
| `rustjay-modulation` — UUID-stable multi-target | Better than the fixed LFO bank for mixer use cases | Medium |

¹ Revised down from Medium: ISF is already prototyped in `examples/isf-example`
(`isf_transpiler.rs`, `isf_effect.rs`, `isf_tab.rs` + 50+ working `.fs` shaders).
The crate is a *promotion*, not a greenfield build.

---

## Repository Map

| Repo | Path | Role |
|------|------|------|
| `rustjay-engine` | `/Users/ac/developer/rust/rustjay-engine` | Main VJ engine — Rust + wgpu + axum |
| `varda` | `/Users/ac/developer/rust/varda` | Feature reference — full standalone VJ app |

Varda is the **feature source of truth**; rustjay-engine is the **architectural
target**. This roadmap ports logic from the former into the latter.

---

## 1. Guiding Principles

| Principle | Application |
|---|---|
| **Evolutionary, not revolutionary** | Every new crate is additive and feature-gated. `cargo build -p delta` must compile and run unchanged at every commit. |
| **Dependency inversion at `rustjay-core`** | New shared vocabulary lands in `rustjay-core`; no new crate may become a dependency *of* `rustjay-core`. |
| **One reason to change per crate** | Mixer ≠ ISF ≠ projection ≠ API ≠ modulation. Each is independently versioned, testable, and droppable. |
| **Prove in an example before promoting to a crate** | The waaaves/isf-example playbook: build as an example, harden, *then* extract the reusable core. |

---

## 2. The Keystone Problem

Everything in Phase B except ISF and modulation is gated behind one decision:

> **`EffectPlugin` describes *one* effect. A mixer is *N* effects + a compositor. How do we nest?**

**Evidence:**
- `crates/rustjay-core/src/plugin.rs` — `type State`, `type Uniforms`,
  `fn shader_source() -> &'static str` are singular throughout.
- `crates/rustjay-render/src/plugin_renderer.rs:8` — `PluginRenderer<P: EffectPlugin>`
  is `pub(crate)`, generic over one `P`, owns the pipeline/uniform/bind-group
  lifecycle, and is instantiated once by the app loop.
- `EffectPlugin::render()` (plugin.rs:288–302) **does** receive
  `&mut encoder, device, queue, render_target_view` — so a plugin *can* own
  arbitrary internal GPU state. `waaaves` proves this with ring buffers in `prepare()`.

**Decision (confirmed): Option B — factor an object-safe `EffectInstance` trait.**

| Option | Description | Verdict |
|---|---|---|
| A. Mixer-as-plugin, channels hand-rolled | Re-implements all of `PluginRenderer`; channels can't reuse arbitrary effects | ❌ dead-end |
| **B. Factor `EffectInstance` trait** | Extract render machinery into a public, object-safe instance wrapping any effect; mixer holds `Vec<Box<dyn EffectInstance>>` | ✅ **chosen** |
| C. New `Compositor` super-trait | Forks the engine's mental model; two parallel code paths forever | ❌ over-engineered |

Option B is the smallest change that lets the mixer compose *existing* effects
(including ISF channels). It pays for itself: transitions, A/B preview, and
sub-mixes all fall out of the same abstraction.

---

## 3. Target Crate Dependency Graph

```
                       rustjay-core
 (EffectPlugin, EffectInstance/EffectInput/RenderTarget, ModulationGraph, EngineState)
              │            │             │            │
        ┌─────┘      ┌─────┘       ┌─────┘      ┌─────┘
   rustjay-render  rustjay-isf  rustjay-modulation  rustjay-control
        │              │             │                  │
        └──────┬───────┴──────┬──────┘                  │
         rustjay-mixer   rustjay-projection              │
               │              │                          │
               └──────┬───────┴──────────────┬───────────┘
                      │                       │
                rustjay-engine (facade) ── rustjay-api
                      │
              examples/varda  ← assembles everything into the full VJ app
```

**Invariant:** all arrows point toward `rustjay-core`. No cycles.
`rustjay-api` and `rustjay-projection` are leaf-adjacent and **off by default**.

---

## 4. Phased Work Breakdown

Each phase ends in a **green workspace**: `cargo check --workspace && cargo test -p delta`.

### Phase B0 — `EffectInstance` factoring *(prerequisite, ~1 week)* — **B0.1/B0.2 DONE**

The true keystone, currently hidden inside the "High" mixer estimate. Unblocks
mixer, transitions, and any future composition.

| Task | Status | File(s) | Acceptance |
|---|---|---|---|
| B0.1 | ✅ **done** | `pub trait EffectInstance` (object-safe) + `EffectInput`, `RenderCtx`, `RenderTarget` in `rustjay-core/src/instance.rs`. | `dyn`-compatible; object-safety unit test passes (`cargo test -p rustjay-core instance`). |
| B0.2 | ✅ **done** | `EffectNode<P>` wrapper + `impl EffectInstance` and `PluginRenderer::render_to_view` in `rustjay-render/src/{instance.rs,plugin_renderer.rs}`. | Any single- **and** multi-pass `EffectPlugin` drives through the trait; render/graph bodies DRY-extracted into shared `render_core`/`run_single_pass`/`run_graph`. delta/flux/sputnik/waaaves/isf-example all compile unchanged. |
| B0.3 | ✅ **done** | `WgpuEngine::render` drives the root effect through the `EffectInstance`-aligned API (`render_to_view` with `EffectInput` slice + target size). Removed the now-redundant wrapper `PluginRenderer::render`. `rustjay-render/src/renderer.rs`. | Engine's production hot path runs the exact slice/`RenderTarget` API a mixer uses; byte-identical (same `render_core`); all 6 examples compile + `delta --release` clean. |
| B0.4 | ⏳ **deferred (off B3 critical path)** | Engine holds a true `Box<dyn EffectInstance>` root (enables a mixer/non-`EffectPlugin` as the engine root). Requires moving `P::State` ownership into the engine. `rustjay-engine/src/app/{mod.rs,events.rs}`, `rustjay-render/src/renderer.rs`. | A `dyn EffectInstance` runs as engine root; GUI/preset state access rerouted; all examples unchanged. |

**B0.1/B0.2 notes (2026-06-03):**
- A bare `impl EffectInstance for PluginRenderer<P>` is impossible — the renderer
  doesn't own `P::State` (the engine's `App` does, `app/mod.rs:178`). The adapter
  is `EffectNode<P>` = renderer + state + label.
- `EffectInput` carries an optional raw `wgpu::Texture` so the custom
  `EffectPlugin::render` hook (ring buffers / feedback copies) works off slices.
- `RenderTarget { view, size }` was added to the trait because multi-pass effects
  need target dimensions to size intermediates (a `TextureView` can't report its size).
- The DRY extraction means `render()` (wrapper-sourced) and `render_to_view()`
  (slice-sourced) share one body via a private `FrameInputs` struct — so multi-pass
  is covered, not deferred.

**B0.3/B0.4 finding (2026-06-03):** the original B0.3 conflated two things. After
B0.2's DRY extraction the engine's render already funnels through the shared
`render_core`, so B0.3 reduced to switching the call site onto the slice/`RenderTarget`
API (done, byte-identical). The *literal* "engine holds a `dyn EffectInstance` root"
is blocked by the `Send + 'static` bound on `EffectInstance` (required so the mixer
can `Box` its channels): a trait object must **own** its state, but the engine keeps
`app_state: P::State` on `App`, shared with the GUI (downcast to `Any`) and presets.
A borrowing adapter can't be `'static`. That state-ownership move is split out as **B0.4**.

**Crucially, B0.4 is _not_ required for the mixer (B3).** `EffectPlugin::render`'s
custom hook already hands a plugin `&mut encoder`, `device`, `queue`, and
`vertex_buffer` — everything needed to build a `RenderCtx` and drive child
`EffectInstance`s. So a `Mixer` can be the engine root **today** by implementing
`EffectPlugin` (passthrough shader + custom `render()` that composites channels),
with zero further engine changes. B0.4 is a cleanliness/ergonomics improvement, not
a B3 blocker.

**Risk:** medium (pure refactor with a strong oracle). **Mitigation:** stand up a
visual regression snapshot (reuse the `egui_kittest` approach from varda's
`tests/ui_snapshots.rs`) *before* B0.4; existing examples must produce
byte-identical output.

**B0.2 invariant (carry into B0.3):** `run_single_pass`/`run_graph` share the
`cached_texture_bind_group` / `cached_pass_texture_gens` fields. This is safe only
because a given `PluginRenderer` is driven by exactly one path. When B0.3 picks a
path per effect, never alternate `render` and `render_to_view` on the same
renderer, or the generation-keyed cache thrashes.

---

### Phase B1 — `rustjay-isf` *(Low–Medium, parallel with B0)*

ISF is a single-effect concern; it implements the *existing* `EffectPlugin` and
needs nothing from B0. Already working in `examples/isf-example`.

| Task | Source of truth | Acceptance |
|---|---|---|
| B1.1 | ✅ **done** | Create `crates/rustjay-isf`; move `isf_transpiler.rs` + `isf_effect.rs` from the example. | Crate compiles; `isf_transpiler.rs` + `isf_effect.rs` + `passthrough.wgsl` moved; tests pass (6/6). |
| B1.2 | ✅ **done** | Hot-reload via mtime polling (existing mechanism preserved; `notify` upgrade is a future enhancement). | Editing a `.fs` file live-reloads without restart. |
| B1.3 | ✅ **done** | ISF metadata → `Vec<ParameterDescriptor>` bridge extracted as reusable free functions in `params.rs`. | `isf_inputs_to_parameters()` + `isf_inputs_to_default_values()` bridge ISF `INPUTS` to engine UI + OSC/MIDI/Web targets. |
| B1.4 | ✅ **done** | Reduce `isf-example` to a thin consumer of the crate. | Example reduced to `main.rs` (launcher) + `isf_tab.rs` (UI); all core logic lives in `rustjay-isf`. |

**Risk:** `shaderc` needs a C++ toolchain (known Linux/Pi pain). **Mitigation:**
strictly feature-gate; document the toolchain; spike `naga`'s native GLSL
frontend as a `shaderc`-free fallback (1 day, decide).

---

### Phase B2 — `rustjay-modulation` *(Medium, parallel with B0/B1)*

Both mixer and API want the richer model. Today `rustjay-core` has `LfoBank` +
`RoutingMatrix` + a fixed `ModulationTarget` enum. Varda has UUID-stable sources
with `HashMap<param, Vec<ParamModulation>>` multi-target assignments.

| Task | Source of truth | Acceptance |
|---|---|---|
| B2.1 | Port `ModulationSourceEntry` (UUID) + assignment model into `rustjay-core` (or a new `rustjay-modulation` re-exported by core). `varda/src/internal/modulation/{engine,sources}.rs`. | Sources survive serialize→deserialize with stable UUIDs (unit test). |
| B2.2 | Adapter: existing `LfoBank` + `AudioRoutingState` become *sources* in the new model. `rustjay-core/src/{lfo,routing}.rs`. | waaaves' 8 LFOs keep working; no behavior change. |
| B2.3 | `O(1)` tick path (port the `uuid_to_idx` cache). `varda/.../engine.rs:28`. | No per-frame allocation in `update()` (flamegraph). |
| B2.4 | **Tempo sync for `ModulationSource::LFO`**. The old `Lfo` has `tempo_sync` + beat divisions + quantum-boundary phase snap; varda's `ModulationSource::LFO` does not. | `ModulationEngine::update()` accepts BPM + beat phase; LFOs snap phase and compute frequency from beat divisions exactly like the old `LfoBank`. |

**Status (2026-06-03):** B2.1–B2.3 landed and committed. B2.4 is a known gap — the adapter (B2.2) converts tempo-sync LFOs to a fixed Hz at snapshot time, so they don't re-sync to BPM changes or snap on bar boundaries.

**Risk:** low–medium (touches every modulation consumer). **Mitigation:** ship
the adapter (B2.2) so old `LfoTarget` code keeps compiling; deprecate, don't delete.

---

### Phase B3 — `rustjay-mixer` *(High, needs B0; benefits from B1+B2)* — **T01–T19 DONE**

The keystone feature. A mixer is `Vec<Channel>` where each `Channel` holds a
`Box<dyn EffectInstance>`, plus crossfader, blend modes, master effect chain, and
beat-synced transitions.

> **Spec:** `examples/mixer/.spec/{requirements,design,tasks}.md` — 11 requirement
> groups (EARS), full type/render-flow design, and 19 tasks (T01–T19). Built
> directly on the landed B0 `EffectInstance` foundation.

**Status (2026-06-03):** T01–T19 landed and committed. `cargo run -p mixer` runs a
2-channel mixer with crossfader, blend modes, master chain, auto/beat-sync/sequenced
transitions, modulatable params, and preset save/load. The crate is feature-gated
(off by default), clippy-clean, and `delta`/`flux`/`sputnik`/`waaaves` compile
unchanged. **Remaining:** T13 (UUID-modulation) is blocked on Phase B2; T19's GPU
flamegraph verification is a hardware follow-up (allocation-free path verified by
construction). The crate is ready to promote from `examples/mixer` per §5.

| Task | Source of truth | Acceptance |
|---|---|---|
| B3.1 | `Channel` = `EffectInstance` + opacity + blend mode. `varda/src/internal/channel/`, `mixer/mod.rs:19`. | A 2-channel mixer composites two `delta` instances. |
| B3.2 | `CompositeBlitPipeline` (all blend modes via uniform). `varda/src/internal/mixer/render.rs:59`, `renderer/blit.rs`. | All varda blend modes reproduced; visual parity. |
| B3.3 | Crossfader + `AutoCrossfade` + `BeatSyncCrossfade`. `varda/src/internal/mixer/transition.rs`. | Beat-synced crossfade lands on the beat (engine BPM). |
| B3.4 | Master effect chain (`Vec<EffectInstance>` post-composite). `mixer/mod.rs:53` ping-pong. | N master effects chain correctly via ping-pong. |
| B3.5 | `TransitionSequence` / `SequencerState`. `mixer/transition.rs`. | Sequenced transitions play back. |
| B3.6 | Mixer itself implements `EffectInstance` → composable / headless / projectable. | Mixer output feeds projection (B4) and NDI out unchanged. |

**Risk:** high (most surface area). **Mitigation:** build as `examples/mixer`
first (mirror the waaaves playbook — write `.spec/{requirements,design,tasks}.md`),
harden, *then* extract to the crate. Gate behind a `mixer` feature.

---

### Phase B4 — `rustjay-projection` *(High, deferred to Phase C)*

**Status: ⏸️ Deferred.** Architecture explored (see plan in `.kimi/plans/speed-killer-frost-falcon.md`) but not implemented. Will be tackled after B5.

Output post-processor: consumes the final composited `TextureView`, warps /
blends / slices it to projector outputs. Architecturally independent of the
mixer — it operates on *any* `EffectInstance` output.

| Task | Status | Source of truth | Acceptance |
|---|---|---|---|
| B4.1 | ⏸️ deferred | `ProjectionStage` trait: `(input_view) → projector outputs`. `varda/src/internal/renderer/{dome,warp,edge_blend,slicer}.rs`. | Identity stage passes frame through unchanged. |
| B4.2 | ⏸️ deferred | Dome + warp + edge-blend pipelines. | Visual parity with varda. |
| B4.3 | ⏸️ deferred | Surface import (DXF/SVG → mesh). `varda/src/internal/surface/{import,detect}.rs`. | A varda stage file imports and renders. |
| B4.4 | ⏸️ deferred | Multi-output window management. `varda/.../renderer/subprocess.rs`, winit. | 2 projector windows, independently warped. |

**Risk:** high (GPU + multi-window + file formats). **Mitigation:** stage-by-stage;
each `ProjectionStage` is independently testable with a known input texture and a
snapshot.

---

### Phase B5 — `rustjay-api` *(Medium, after B3 for full surface)*

Extract varda's axum REST/OpenAPI layer as an optional crate. The existing
HTML/WS web UI becomes the *default minimal* implementation; `rustjay-api` is the
full-featured opt-in.

| Task | Source of truth | Acceptance |
|---|---|---|
| B5.1 | `command_tx` + `engine_state: Arc<RwLock>` transport (port `SharedState`). `varda/src/usecases/api/mod.rs:21`. | Commands round-trip via channel; handlers never hold the engine. |
| B5.2 | Map varda `EngineCommand` ↔ rustjay `InputCommand`/`OutputCommand`/etc. `rustjay-core/src/state.rs:16`. | Existing engine commands all reachable over HTTP. |
| B5.3 | Projection DTOs + diff cache (reuse AUDIT_ROADMAP Task 3.4 diff-tracking). `varda/.../api/{projection,ws}.rs`. | WS pushes only changed params. |
| B5.4 | OpenAPI/Swagger (`utoipa`). | `/swagger-ui` serves the full schema. |
| B5.5 | **Inherit the AUDIT_ROADMAP network model** (Tasks 1.3, 1.4-R). | LAN bind is the **default** (trusted FOH LAN); `--bind 127.0.0.1` + `--web-token` are **opt-in** hardening for shared/untrusted networks. DOM-XSS escaping (1.3) is always on. CI: with token enabled, 401 without it. |

**Risk:** medium. The network model follows Task 1.4-R: LAN-by-default is correct
for the show use case; the opt-in `--bind`/`--web-token` hardening and the
always-on DOM-XSS escaping are what B5.5 must carry — don't reinvent them, and
don't regress to localhost-by-default.

---

## 5. Implementation Order & Parallelism

```
Week 1-2:  B0 (EffectInstance refactor)  ──┐ critical path
           B1 (ISF crate)        ─ parallel │
           B2 (modulation)       ─ parallel │
                                            │
Week 3-5:  B3 (mixer, as example first) ────┤ needs B0
                                            │
Week 6-7:  B5 (API)              ───────────┘ needs B3 surface
           Promote examples/mixer → crate
           examples/varda assembles the full app
```

**Critical path:** B0 → B3 → B5. B1 and B2 are off the critical path and are
done. B4 is deferred to Phase C.

---

## 6. Risk Register

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | B0 refactor regresses existing examples | Med | High | Visual snapshot oracle *before* refactor; `dyn` wrapping is mechanical. |
| R2 | `shaderc` C++ toolchain breaks Linux/Pi builds | Med | Med | Feature-gate; spike `naga` native GLSL frontend as fallback. |
| R3 | Mixer-inside-mixer recursion blows bind-group limits | Low | Med | Cap nesting depth; composite always flattens to one output texture. |
| R4 | API deployed on an *untrusted* network without opting into hardening | Med | High | LAN-default is fine on an isolated show LAN (Task 1.4-R); document the trust assumption + opt-in `--bind`/`--web-token` in GUIDE; always-on DOM-XSS escaping (1.3). |
| R5 | Modulation migration breaks waaaves' 8 LFOs | Low | Med | Adapter layer (B2.2); deprecate don't delete; serialize round-trip test. |
| R6 | Scope creep (HAP, scenes, keymap, sysmon, recording also exist in varda) | High | Med | This roadmap is explicitly the 5 Phase-B items; everything else is Phase C backlog. |

---

## 7. Definition of Done (Phase B)

1. `examples/varda` exists in the workspace and assembles `rustjay-mixer` +
   `rustjay-isf` + `rustjay-projection` + `rustjay-api` + `rustjay-modulation`
   into a VJ app at feature parity with standalone varda for the five extracted domains.
2. `cargo build -p delta` (and flux / sputnik / waaaves) compiles and runs
   **unchanged** — proving the additions are non-invasive.
3. Every new crate is feature-gated and **off by default** in `rustjay-engine`.
4. The API follows the Task 1.4-R network model: LAN-by-default for shows, opt-in `--bind`/`--web-token` hardening, always-on DOM-XSS escaping.
5. `cargo test --workspace` green; mixer/projection have snapshot tests;
   modulation has serialize round-trip tests.

---

## 8. Phase C Backlog (out of scope, recorded for continuity)

Varda capabilities **not** in Phase B, captured so they aren't lost:

- HAP codec video playback (`varda/src/internal/video/hap.rs`, `renderer/hap_convert.rs`)
- Scene system (`varda/src/internal/scene/`)
- Keymap / hotkey layer (`varda/src/internal/keymap/`)
- Recording / capture (`varda/src/internal/recording/`)
- System monitor (`varda/src/internal/sysmon/`)
- Camera-based auto-mapping (recently added; see varda PR #14)

---

## Briefing Template for Sub-Agents

```
You are working on rustjay-engine at /Users/ac/developer/rust/rustjay-engine.

Your task is Task [B?.?]: [Title] from PHASE_B_ROADMAP.md.

Source of truth (varda): [path from the task row]
Acceptance: [copy the Acceptance cell]

Constraints:
- Do not break `cargo build -p delta` (single-plugin model must stay intact).
- New crates are feature-gated and off by default.
- Run `cargo check --workspace` and `cargo clippy -- -D warnings` before reporting done.
```
