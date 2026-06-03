# Mixer — Implementation Tasks

**Version:** 1.0
**Status:** Draft (Phase 3)

Scope tags: 🔧 `ENGINE` = `crates/rustjay-*` · 📦 `MIXER` = `examples/mixer/`
Dependencies: `needs: [T##, ...]`. Tasks with no `needs:` can start immediately.

**Prerequisite (done):** B0.1–B0.3 are landed and verified — `EffectInstance` /
`EffectNode` / `EffectInput` / `RenderTarget` exist, and the engine's render path
runs the slice/`RenderTarget` API.

**Mixer-as-engine-root strategy:** the `Mixer` becomes the engine root by
implementing `EffectPlugin` (passthrough shader + a custom `render()` that builds a
`RenderCtx` from the hook's `encoder`/`device`/`queue`/`vertex_buffer` and drives
each channel's `EffectInstance`, then composites). This needs **no** further engine
change — B0.4 (true `dyn EffectInstance` root) is an optional later ergonomics
improvement, not a blocker (see PHASE_B_ROADMAP §B0).

---

## Group 0 — Crate scaffold

### T01 🔧 ENGINE — Create `rustjay-mixer` crate ✅ DONE (2026-06-03)
**File:** `crates/rustjay-mixer/{Cargo.toml,src/lib.rs}`, workspace `Cargo.toml`

- [x] Add `crates/rustjay-mixer` to workspace members + `workspace.dependencies`
- [x] Deps: `rustjay-core`, `rustjay-render`, `wgpu`, `bytemuck`, `log`, `serde`
- [x] Add a `mixer` feature to `rustjay-engine`, off by default (`mixer = ["dep:rustjay-mixer"]`)
- [x] Stub `Mixer`, `Channel`, `BlendMode` with docs (`#![warn(missing_docs)]`)

**Done:** `cargo test -p rustjay-mixer` green; engine builds with and without `--features mixer`; `delta`/`waaaves` unaffected.

---

## Group 1 — Compositing

### T02 🔧 ENGINE — `BlendMode` + `composite.wgsl` ✅ DONE (2026-06-03)
**File:** `crates/rustjay-mixer/src/{blend.rs,composite.wgsl}`
**Implements:** REQ-02.1, REQ-02.2

- [x] Ported the 15 `BlendMode` variants + indices + `short_name`/`all()` from Varda
- [x] `composite.wgsl`: samples source + dest, branches on blend index, self-contained vs+fs
- [x] naga WGSL front-end validates the shader (unit test, no GPU)

**Done:** `composite_shader_validates` + `indices_are_contiguous_and_match_order` tests pass.

### T03 🔧 ENGINE — `CompositePipeline` ✅ DONE (2026-06-03, caching deferred)
**File:** `crates/rustjay-mixer/src/composite.rs`
**Needs:** T02 · **Implements:** REQ-02.3 (REQ-11.1 deferred to T19)

- [x] Pipeline + bind layout (`@group(0)`: sampler, source, dest, params) — matches Varda
- [x] `blend(device, encoder, source, dest, out, opacity, mode, vertex_buffer)` — REPLACE to a third texture (ping-pong; you can't sample the render target)
- [ ] Generation-keyed bind-group cache — **deferred to T19** (current `blend` allocates a uniform+bind group per call; correct but per-frame; documented `TODO`)

**Done:** pipeline compiles; shader validates. GPU pixel-level blend test deferred to integration (needs a device).

> **Design reconciliation:** the design doc's "accumulation read via `LoadOp::Load`"
> is **wrong** for a shader compositor — you cannot sample the texture you render
> into. The implemented (and Varda-proven) approach samples `source` + `dest` and
> writes to a third texture, so the mixer ping-pongs two accumulation textures.
> design.md §6 should be updated to match when T07 lands.

---

## Group 2 — Channels

### T04 🔧 ENGINE — `Channel` + per-channel render ✅ DONE (2026-06-03)
**File:** `crates/rustjay-mixer/src/lib.rs`
**Needs:** T01 · **Implements:** REQ-01.1, REQ-01.3, REQ-01.4, REQ-11.2

- [x] `Channel` gains `texture: Option<Texture>` / `ping: Option<Texture>` + `LastOutput` tracking
- [x] Allocate channel textures once; reallocate only on resize via `Channel::ensure_size`
- [x] Render the channel effect into `texture` via `EffectInstance::render_to`

**Done:** `Channel::render` drives the channel effect into its texture. Textures are allocated
lazily on first render and resized when the mixer target size changes.

> **Review fix (2026-06-03):** using `rustjay-render` here makes the crate's test
> binary link `Syphon.framework` on macOS. Added `crates/rustjay-mixer/build.rs` to
> re-emit the Syphon `-rpath` (the link-arg doesn't propagate downstream — see
> `rustjay-render/build.rs`). Without it `cargo test -p rustjay-mixer` aborts at load
> time (dyld). With it, 7 tests pass.

### T05 🔧 ENGINE — Per-channel effect chain ✅ DONE (2026-06-03)
**File:** `crates/rustjay-mixer/src/lib.rs`
**Needs:** T04 · **Implements:** REQ-01.5, design Q2

- [x] `chain: Vec<Box<dyn EffectInstance>>` on `Channel`
- [x] `run_chain(effects, src, ping)` ping-pong helper extracted and shared with master chain

**Done:** `Channel::render` runs the effect chain via `run_chain`, tracking the final output
with `LastOutput` so `output_texture()` returns the correct texture for compositing.

### T06 🔧 ENGINE — Dynamic add/remove channels ✅ DONE (2026-06-03)
**File:** `crates/rustjay-mixer/src/lib.rs`
**Needs:** T04 · **Implements:** REQ-01.2

- [x] `add_channel` clamps at 8 channels max; `remove_channel` clamps at 1 channel min
- [x] New channels get textures lazily on next `render_to` via `ensure_resources`

**Done:** Runtime add/remove is bounded; unit test `channel_count_clamped` verifies limits.

---

## Group 3 — Mixer composition & EffectInstance

### T07 🔧 ENGINE — `Mixer::render_to` (composite + master) ✅ DONE (2026-06-03)
**File:** `crates/rustjay-mixer/src/lib.rs`
**Needs:** T03, T05 · **Implements:** REQ-01.4, REQ-02.3, REQ-06, REQ-08.1, REQ-08.2, REQ-11.3

- [x] `impl EffectInstance for Mixer` — composability/nesting (REQ-08.1)
- [x] Render channels → composite (skip eff opacity `< 0.001`) → master chain → blit to target
- [x] Empty master chain = passthrough via `run_chain` early return (no extra pass)

**Done:** Full render pipeline implemented with ping-pong compositing (`acc_a`/`acc_b`),
per-channel effect chains, master chain, and final blit. GPU pixel-level verification
deferred to T17 headless test (needs environment with working `wgpu::Device`).

### T07b 🔧 ENGINE — `MixerPlugin` engine-root wrapper ✅ DONE (2026-06-03)
**File:** `crates/rustjay-mixer/src/plugin.rs`
**Needs:** T07 · **Implements:** REQ-08.2, REQ-08.3

- [x] `MixerPlugin` implements `EffectPlugin` with a dummy passthrough shader
- [x] Custom `render()` hook builds `RenderCtx` and calls `Mixer::render_to`; returns `true`
- [x] `parameters()` delegates to `Mixer::parameters()`
- [x] `Mixer` is held inside `std::sync::Mutex<Mixer>` so `MixerPlugin` satisfies `EffectPlugin: Sync`
  (the `dyn EffectInstance` trait bound is `Send` only; `Mutex` is the minimal safe adapter).

**Done:** `cargo check -p rustjay-engine --features mixer` compiles; `MixerPlugin` can be
passed as the engine root plugin.

### T08 🔧 ENGINE — Parameter aggregation
**File:** `crates/rustjay-mixer/src/lib.rs`
**Needs:** T07 · **Implements:** REQ-08.4

- [ ] Aggregate `parameters()` from self + channel/master effects, namespaced by UUID

**Done when:** a nested channel effect's parameter is reachable via OSC by `ch:<uuid>/...`.

---

## Group 4 — Crossfader & transitions

### T09 🔧 ENGINE — Crossfader + effective opacity
**File:** `crates/rustjay-mixer/src/lib.rs`
**Needs:** T07 · **Implements:** REQ-02.4, REQ-03.1, REQ-03.2

- [ ] `crossfader: f32`; 2-channel opacity derivation; >2-channel direct opacity
- [ ] Register crossfader as a modulatable parameter

**Done when:** moving the crossfader cross-blends two channels; an LFO can drive it.

### T10 🔧 ENGINE — Auto + beat-synced crossfade
**File:** `crates/rustjay-mixer/src/crossfade.rs`
**Needs:** T09 · **Implements:** REQ-04.1–04.4

- [ ] `AutoCrossfade` (4 easings) + `BeatSyncCrossfade` (waits for beat boundary)
- [ ] Use `engine.effective_bpm()` for duration; snap to target on completion

**Done when:** a beat-synced crossfade starts on the beat and lands exactly on target.

### T11 🔧 ENGINE — Transition sequencer
**File:** `crates/rustjay-mixer/src/sequencer.rs`
**Needs:** T10 · **Implements:** REQ-05.1–05.3

- [ ] `SequencerState`, `TransitionStep`, `StepKind { Crossfade, Hold, Effect }`
- [ ] Playback with per-step beat durations; loop flag; manual input stops sequence

**Done when:** a 3-step sequence plays back and loops; manual crossfade interrupts it.

---

## Group 5 — Modulation

### T12 🔧 ENGINE — Modulation wiring
**File:** `crates/rustjay-mixer/src/lib.rs`
**Needs:** T09 · **Implements:** REQ-07.1, REQ-07.3

- [ ] Drive mixer params from engine LFO/audio routing (fallback path)

**Done when:** an LFO modulates channel opacity and the crossfader.

### T13 🔧 ENGINE — B2 UUID-modulation integration *(optional, when B2 lands)*
**File:** `crates/rustjay-mixer/src/lib.rs`
**Needs:** T12, Phase B2 · **Implements:** REQ-07.2

- [ ] Use `rustjay-modulation` UUID sources + multi-target assignments
- [ ] Assignments survive preset round-trip

**Done when:** one source modulates ≥2 mixer params and persists across save/load.

---

## Group 6 — GUI (`examples/mixer`)

### T14 📦 MIXER — Mixer tab (channel strips + crossfader)
**File:** `examples/mixer/src/tabs/mixer_tab.rs`
**Needs:** T07 · **Implements:** REQ-09.1

### T15 📦 MIXER — Channel detail tab
**File:** `examples/mixer/src/tabs/channel_tab.rs`
**Needs:** T08 · **Implements:** REQ-09.2

### T16 📦 MIXER — Transition controls tab
**File:** `examples/mixer/src/tabs/transition_tab.rs`
**Needs:** T11 · **Implements:** REQ-09.3

### T17 📦 MIXER — Assemble example app
**File:** `examples/mixer/src/main.rs`
**Needs:** T07b · **Implements:** REQ-08.3

- [ ] Build a 2-channel `MixerPlugin` of two example effects; `run_with_tabs(...)`
- [ ] Verify headless mode (`run_headless`) renders and accepts OSC

**Done when:** `cargo run -p mixer` shows a working 2-channel mixer with crossfader.

---

## Group 7 — Persistence & hardening

### T18 🔧 ENGINE — Preset save/load
**File:** `crates/rustjay-mixer/src/lib.rs`
**Needs:** T07 · **Implements:** REQ-10.1, REQ-10.2, REQ-10.3

- [ ] Serialize topology + per-effect state; bounded deserialization (AUDIT_ROADMAP 2.1)

**Done when:** save→load restores channels, crossfader, blend modes, and per-effect params.

### T19 🔧 ENGINE — Performance pass
**File:** `crates/rustjay-mixer/src/*`
**Needs:** T07 · **Implements:** REQ-11.1–11.4

- [ ] Confirm no per-frame texture/bind-group allocation (flamegraph/heaptrack)
- [ ] Document and assert the single-render-path invariant per `EffectInstance`

**Done when:** steady-state mixing shows zero per-frame GPU allocations.

---

## Definition of Done (Mixer / Phase B3)

1. `cargo run -p mixer` runs a 2-channel mixer with crossfader, blend modes, and a master effect.
2. The mixer works as a `dyn EffectInstance`: previews, outputs over NDI/Syphon, and can nest.
3. Beat-synced crossfades and a transition sequence work against the engine BPM.
4. `cargo build -p delta` (and flux/sputnik/waaaves) compiles unchanged.
5. `rustjay-mixer` is feature-gated, off by default; `cargo test --workspace` green.
