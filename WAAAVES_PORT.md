# WAAAVES Port

Full port of `rustjay-waaaves` into `rustjay-engine` as `examples/waaaves`.

> **This document is a status overview. The authoritative spec lives at:**
> `examples/waaaves/.spec/requirements.md` — 43 EARS-notation requirements
> `examples/waaaves/.spec/design.md`        — types, bind layouts, state machines
> `examples/waaaves/.spec/tasks.md`         — 47 tasks with done criteria

**Goals:**
- All three processing blocks as separate imgui tabs (Block 1 / Block 2 / Block 3)
- All shaders extracted to `.wgsl` files and reviewed for performance
- 8 LFOs (up from 3) with full waaaves parameter coverage
- Preview-window pixel-pick color picker for keying
- Legacy preset import from old JSON format

---

## Decisions

| Topic | Decision |
|---|---|
| Dual video input | Extend `PassInput` with `SecondInput` variant in engine |
| LFO count | Expand `LfoBank` from 3 to 8 |
| Color picker | Preview-window click → GPU readback → set as active key color |
| Preset compat | Write a migration layer that reads old `Block1Params`/`Block2Params`/`Block3Params` JSON |

---

## Architecture

### Render Graph

```
Engine input (ch1 tex, ch2 tex) ─┐
Feedback tex ─────────────────────┤  Pass 0 (Block A)  →  intermediate_a
                                   │
intermediate_a ───────────────────┤  Pass 1 (Block B)  →  intermediate_b
Feedback tex ─────────────────────┘

intermediate_a + intermediate_b ──   Pass 2 (Block C)  →  output
```

- Pass 0 consumes `ch1`, `ch2`, and the ping-pong `fb1` texture.
- Pass 1 consumes `intermediate_a` and `fb2`.
- Pass 2 consumes `intermediate_a`, `intermediate_b`, and produces the final frame.
- All three passes share a single `WaaavesUniforms` mega-struct (all params packed flat); each shader only reads the fields it needs.

### Uniform Strategy

`Block1Uniforms` is ~650 bytes; `Block2Uniforms` is ~600 bytes; `Block3Uniforms` is ~500 bytes. Rather than three separate Rust types (which the single `EffectPlugin::Uniforms` associated type cannot express without boxing), define one flat `WaaavesUniforms` covering all three blocks (~1.8 KB total, well within the 65 536-byte wgpu uniform limit). `build_pass_uniforms(pass_index)` fills the same struct regardless of pass; each shader's `struct WaaavesUniforms` declaration only lists the fields it uses.

### State

```rust
struct WaaavesState {
    block1: Block1Params,     // ported from waaaves params module
    block2: Block2Params,
    block3: Block3Params,
    active_key_target: KeyTarget,  // which key color the picker populates
}
```

`LfoState` lives in `EngineState`; LFO targets are registered via `EffectPlugin::parameters()`.

---

## Phase Plan

### Phase 0 — Inventory & Gaps (no code)

1. Audit existing `examples/waaaves/` stub against the three full block pipelines.
2. List every texture binding per pass (see below).
3. Document all ~200 waaaves parameters that need `ParameterDescriptor` entries.
4. Identify which parameters are sensible LFO targets (geometry floats, HSB floats, mix amounts).

**Binding requirements per pass:**

| Pass | Textures needed |
|---|---|
| Block A | `ch1_tex`, `ch2_tex`, `fb1_tex`, `temporal_tex` + their samplers |
| Block B | `block_a_out`, `fb2_tex`, `temporal_tex` + samplers |
| Block C | `block_a_out`, `block_b_out` + samplers |

This exceeds what `RenderGraph` currently wires, so Phase 1 must precede shader work.

---

### Phase 1 — Engine Extensions

**1.1 — `PassInput::SecondInput`**

In `crates/rustjay-core/src/plugin.rs`:
```rust
pub enum PassInput {
    EngineInput,
    SecondInput,     // new: engine's second video input slot
    PreviousPass,
    Feedback,
}
```
The engine render loop must bind this at `@group(0) @binding(2/3)` when present (currently those slots are used for `feedback_tex`; the layout will need an extra set of bindings for dual-input passes, or a dedicated `@group(2)`).

**Suggested bind group layout for dual-input passes:**
- `@group(0)` — `ch1_tex`, `ch1_sampler`, `ch2_tex`, `ch2_sampler`
- `@group(1)` — `uniforms`
- `@group(2)` — `fb_tex`, `fb_sampler`, `temporal_tex`, `temporal_sampler`

This requires the engine's `render_graph` executor to detect dual-input passes and switch bind group layouts accordingly.

**1.2 — Expand `LfoBank` to 8**

In `crates/rustjay-core/src/lfo.rs`:
- Change `lfos: [Lfo; 3]` → `lfos: Vec<Lfo>` (or `[Lfo; 8]` — Vec preferred for flexibility).
- Update `LfoBank::new()` to initialize 8 LFOs with sensible defaults.
- Update `LfoBank::update()`, `get_modulations()`, reset helpers.
- Update engine GUI LFO panel to render 8 rows instead of 3.
- Remove `#[deprecated]` annotations on `get_hsb_modulations` / `apply_to_hsb` only after confirming no other consumers remain; otherwise leave them deprecated but working.

**1.3 — Preview pixel-pick API**

Add to `EngineState`:
```rust
pub pick_request: Option<[f32; 2]>,    // UV to sample (set by GUI on click)
pub picked_color: Option<[f32; 3]>,    // RGB result after GPU readback
```

In the engine render loop, after the frame is composed:
- If `pick_request` is `Some(uv)`, map the output texture (via a `wgpu::Buffer` with `MAP_READ`), read the pixel at `(uv.x * width, uv.y * height)`, write `[r, g, b]` to `picked_color`, clear `pick_request`.

The waaaves GUI watches `picked_color` and routes it to whichever key parameter is currently active (`active_key_target`).

---

### Phase 2 — Shader Migration

Extract the inline WGSL embedded in `rustjay-waaaves/src/engine/pipelines/block1.rs` (and block2, block3) into standalone files. Replace the current stub shaders in `examples/waaaves/src/shaders/`.

**Target files:**
```
examples/waaaves/src/shaders/
  block_a.wgsl    ← full Block 1 (ch1, ch2, fb1, temporal, all geometry/color/key ops)
  block_b.wgsl    ← full Block 2 (block2 input, fb2, all geometry/color/key ops)
  block_c.wgsl    ← full Block 3 (geo, colorize bands, matrix mixer, final mix)
```

**Shader performance review checklist:**

| Item | Status in original | Action |
|---|---|---|
| `blur_and_sharpen` early exit (`< 0.001` guards) | Present | Verify retained |
| HSB conversion skip when no HSB ops needed | Present (`needs_hsb` flag) | Verify retained |
| `pow(hsb, attenuate)` on vec3 | Present — GPU-native, acceptable | Keep |
| `do_kaleidoscope` unconditional path | Branches on `segments <= 0.0` | Add `if kaleidoscope > 0.001` guard at call site to skip entirely |
| `do_rotate` on every pixel | No guard | Add `if rotate != 0.0` guard |
| Shear matrix applied unconditionally | No guard | Add identity-matrix check |
| `unsafe` byte transmute for uniform upload | Present in waaaves | **Remove** — use `bytemuck::bytes_of()` in engine |
| Blur is 8-tap box blur | Correct — simple and fast | Keep |
| Sharpen is 8-tap brightness average | Correct | Keep |
| Mix modes use `switch` statement | Correct | Keep |
| Texture sampling in coordinate space vs UV space | Pixel coords converted at call site | Standardize on UV throughout to avoid divide-by-resolution in shaders |

**WGSL validation:** run `naga validate` on each extracted shader before integration.

---

### Phase 3 — Parameters & State

**3.1 — `WaaavesState`**

Port `Block1Params`, `Block2Params`, `Block3Params` directly from `rustjay-waaaves/src/params/mod.rs`. Remove the redundant `_x/_y/_z` scalar duplicates of Vec3 fields (those existed for per-component LFO modulation in the old system; in the engine the `ParameterDescriptor` + `custom_params` system handles this cleanly).

```rust
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct WaaavesState {
    block1: Block1Params,
    block2: Block2Params,
    block3: Block3Params,
    active_key_target: KeyTarget,
}
```

**3.2 — `WaaavesUniforms` mega-struct**

Concatenate `Block1Uniforms` + `Block2Uniforms` + `Block3Uniforms` into one `#[repr(C)]` struct with `bytemuck::Pod + Zeroable`. All fields use `f32`/`i32`/`u32`; Vec3 fields need 16-byte alignment padding. Annotate clearly which section each pass reads.

**3.3 — `EffectPlugin::parameters()`**

Register every modulatable parameter via `ParameterDescriptor`. Priority LFO targets (register first):

| Group | Key params |
|---|---|
| Block 1 geometry | `ch1_x/y/z_displace`, `ch1_rotate`, `ch1_kaleidoscope_amount` |
| Block 1 color | `ch1_hsb_attenuate_{x,y,z}`, `ch2_mix_amount` |
| FB1 geometry | `fb1_x/y/z_displace`, `fb1_rotate`, `fb1_shear_matrix_{x,y,z,w}` |
| FB1 color | `fb1_hsb_offset_{x,y,z}`, `fb1_hsb_attenuate_{x,y,z}`, `fb1_hue_shaper` |
| Block 2 | Same pattern for fb2 equivalents |
| Block 3 | `block1_colorize_band{1-5}_{x,y,z}`, `final_mix_amount`, matrix mixer diagonals |

All float params → `ParameterDescriptor::float()`. Bool/int enums → omit from LFO targets (engine only modulates float/int types).

**3.4 — `build_pass_uniforms(pass_index)`**

```rust
fn build_pass_uniforms(&self, pass_index: usize, state: &WaaavesState, engine: &EngineState) -> WaaavesUniforms {
    let mut u = WaaavesUniforms::from_state(state, engine);
    // Apply LFO/audio modulations from engine.custom_params
    // (engine already wrote modulated values into custom_params)
    u.apply_custom_params(engine);
    u
}
```

All three passes share the same struct; the shaders simply ignore irrelevant fields.

---

### Phase 4 — GUI Tabs

Three `AnyGuiTab` implementations, passed to `run_with_tabs()`.

**Block1Tab** — sub-sections via `CollapsingHeader`:
- CH1: x/y/z displace, rotate, HSB attenuate, blur/sharpen, kaleidoscope, switches (h/v mirror, flip, hue/sat/bright invert, RGB invert, solarize, posterize), geo overflow, input select
- CH2 Mix: mix amount, mix type, key mode/order/threshold/softness, key color (RGB picker + **pick from preview** button)
- CH2 Adjust: same geometry/color/filter controls as CH1
- FB1 Mix: same key controls, delay time (with BPM sync toggle)
- FB1 Geo: x/y/z displace, rotate, shear matrix, kaleidoscope, h/v mirror/flip, geo overflow
- FB1 Color: HSB offset, HSB attenuate, HSB powmap, hue shaper, posterize, inverts

**Block2Tab** — sub-sections:
- Block 2 Input: geometry + color + filter controls, input select (block1/ch1/ch2), switches
- FB2 Mix: mix amount, mix type, key controls, key color picker
- FB2 Geo + Color + Filters: mirrors FB1 layout
- FB2 Delay time

**Block3Tab** — sub-sections:
- Block 1 Output Geo: displace, rotate, shear, kaleidoscope, mirror/flip, geo overflow
- Block 1 Colorize: on/off toggle, HSB/RGB mode, 5 color bands (each with HSB/RGB color picker + **pick** button)
- Block 1 Filters + Dither
- Block 2 Output Geo + Colorize + Filters (mirrors block 1 section)
- Matrix Mixer: 3×3 R/G/B crosspoint grid
- Final Mix: amount, mix type, overflow, key controls, output dither

**Color picker integration:**
- Each key color row has a `[Pick]` button beside the RGB sliders.
- Clicking `[Pick]` sets `engine.pick_request` to `None` (armed) and `state.active_key_target` to the relevant param.
- The engine preview window shows a crosshair cursor while armed.
- On click in the preview, engine records the UV and triggers GPU readback.
- Next frame, `engine.picked_color` is `Some([r,g,b])` → GUI routes it to the active key target's three float params.

---

### Phase 5 — LFO Tidy-up

The engine's LFO system is mostly correct but needs:

1. **Expand to 8:** change `[Lfo; 3]` → `Vec<Lfo>`, `LfoBank::new()` creates 8. Update serialization (old saves with 3 LFOs deserialize fine since Vec grows with defaults).
2. **Remove deprecated stubs:** `get_hsb_modulations` and `apply_to_hsb` in `LfoState` — check all callers, then delete.
3. **`LfoTarget::all_for()` already handles `Custom` targets** — no change needed, just ensure all waaaves float params are in `parameters()` so the target dropdowns populate.
4. **Phase offset in GUI:** expose `phase_offset` slider (0–360°) in the LFO panel — currently defined on `Lfo` but not shown in GUI.
5. **LFO visualizer:** small waveform preview per LFO row using `get_waveform_value_at()` — optional but useful.
6. **Beat snap:** `update()` already handles quantum boundary snapping. Verify it works correctly with the expanded bank under audio, Link, and ProDJ sources.

---

### Phase 6 — Preset Import

Add `legacy_preset.rs` to the waaaves example:

```rust
pub fn import_legacy_preset(json: &str) -> anyhow::Result<WaaavesState> {
    // Deserialize old { block1: Block1Params, block2: Block2Params, block3: Block3Params }
    // JSON directly into WaaavesState (field names are identical)
    let legacy: LegacyPreset = serde_json::from_str(json)?;
    Ok(WaaavesState {
        block1: legacy.block1,
        block2: legacy.block2,
        block3: legacy.block3,
        active_key_target: KeyTarget::None,
    })
}
```

The old JSON field names in `Block1Params` etc. are already identical to what we'll use in `WaaavesState`, so direct deserialization should work for most fields. Exceptions:
- `fb1_shear_matrix` (was `Vec4`) maps cleanly.
- `ch1_hsb_attenuate` (was `Vec3`) — old presets have both the Vec3 and the `_x/_y/_z` scalars; use the Vec3 and ignore the duplicates.
- `block1_colorize_band{1-5}` (Vec3 + scalar duplicates) — same as above.

Add an **"Import Legacy Preset…"** file picker button in the Presets tab that calls `import_legacy_preset()` and replaces the current state.

---

## File Layout After Port

```
examples/waaaves/
  Cargo.toml
  build.rs
  src/
    main.rs              EffectPlugin impl, WaaavesState, WaaavesUniforms, run_with_tabs()
    block1_tab.rs        Block1Tab: AnyGuiTab
    block2_tab.rs        Block2Tab: AnyGuiTab
    block3_tab.rs        Block3Tab: AnyGuiTab
    legacy_preset.rs     import_legacy_preset()
    params/
      block1.rs          Block1Params (ported, cleaned)
      block2.rs          Block2Params (ported, cleaned)
      block3.rs          Block3Params (ported, cleaned)
      mod.rs
    shaders/
      block_a.wgsl       Full Block 1 shader
      block_b.wgsl       Full Block 2 shader
      block_c.wgsl       Full Block 3 shader

crates/rustjay-core/src/
  lfo.rs                 LfoBank expanded to Vec<Lfo> (8 default)
  plugin.rs              PassInput::SecondInput added
  state.rs               pick_request, picked_color added to EngineState
```

---

## Risks & Open Questions

| Risk | Mitigation |
|---|---|
| Mega-uniform struct exceeds WebGPU limits on some devices | At ~1.8 KB this is fine; document minimum `max_uniform_buffer_binding_size` requirement |
| Second input source: what does ch2 map to? | Add a `ch2_input_type` selector in the Input tab (Syphon server name, webcam index, or NDI source separate from ch1) |
| GPU readback latency (pick-from-preview) | Use async map + poll; result available next frame — show a one-frame delay spinner |
| Temporal delay ring buffer | Manage in `init()`/`prepare()` hooks as a `Vec<wgpu::Texture>` circular buffer; size = max(`fb1_delay_time`, `fb2_delay_time`) + 1 |
| `fb1_delay_time_sync` / beat-synced delay | Convert beat division to frame count using `engine.effective_bpm()` and current FPS each frame |
| Colorize bands use Vec3 in old code | Standardize to `[f32; 3]` arrays internally for `bytemuck` compatibility; expose as `[f32; 3]` in GUI |
| Shader compilation time | Large shaders with many functions may increase startup time; consider `wgpu::ShaderModuleDescriptor { source: ... }` caching or `pipeline_cache` feature |

---

## Implementation Order

```
Phase 1.2 (LFO expand)           ← independent, low risk, unblocks Phase 5
Phase 1.1 (PassInput::SecondInput) ← needed before shader work
Phase 1.3 (pixel pick API)        ← can be done in parallel
Phase 2   (shader extraction)     ← needs Phase 1.1 done
Phase 3   (params + uniforms)     ← can start in parallel with Phase 2
Phase 4   (GUI tabs)              ← needs Phase 3 done
Phase 5   (LFO tidy-up)          ← needs Phase 1.2 + Phase 3
Phase 6   (preset import)         ← independent, low risk
```

Total estimated complexity: **large but tractable** — the shader logic already exists and is correct; this is primarily a structural migration, not a rewrite.
