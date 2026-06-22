# RustJay 404 Port Roadmap

Porting the **RustJay 404** SP-404-style video sampler onto **rustjay-engine**,
assembled as `examples/vp404`.

> Architect planning document. Companion to [`VARDA_PORT.md`](VARDA_PORT.md) and
> [`WAAAVES_PORT.md`](WAAAVES_PORT.md) (the proven "port an app into the engine as
> an example" playbook) and the `rustjay` agent skill at
> [`.agents/skills/rustjay/`](.agents/skills/rustjay/SKILL.md).

---

## 0. What 404 is

A ~21k-LOC **video sampler**: HAP-encoded clips on a 16-pad grid (SP-404 trigger
modes), a polyphonic 16-track step sequencer, a GPU mixer with blend modes +
chroma/luma keying, live sampling (capture → HAP), MIDI/OSC, and
NDI/Syphon/Spout/V4L2 I/O. Architecturally closest to `examples/vjarda`
(mixer-as-root, `render()` override) — **not** a shader effect like
waaaves/shaderglass.

## 1. Design decisions (2026-06-20)

| Decision | Choice |
|---|---|
| **Scope** | MVP-first, then layer features in phases. |
| **UI** | Rebuild in **egui** (`run_with_egui_tabs`) — matches vjarda/shaderglass/delta-egui. The 404 imgui windows are **not** ported. |
| **HAP decode/encode** | **No wrapper crate.** Depend on **`hap-wgpu`** directly (user-owned, `/Users/ac/developer/rust/hap-rs`, published 0.1.0, already wgpu-29). It already exposes the full `HapPlayer` *and* `HapVideoEncoder` + GPU DXT compressor. Edit + publish hap-wgpu when a gap needs filling. |
| **HAP YCoCg/alpha** | hap-wgpu hands back **raw BCn textures**; YCoCg→RGB + dual-plane alpha (HapY / HAP Q Alpha) is the consumer's job. Reference **varda-orig's `hap_convert.wgsl`** (already solves this) and upstream a decode-side convert into hap-wgpu. |
| **Live sampling / encode** | In scope from **Phase 1**. Encoder already exists in hap-wgpu (`HapVideoEncoder`) — wire, don't write. |
| **Name** | App is **VP-404** (our take on the SP-404). Member `examples/vp404` (cargo rejects digit-leading package names); `app_name() = "VP-404"` drives the UI title + `~/.config/rustjay/VP-404.json`. |
| **Channel/pad count** | **Exposed as a setting**, not fixed at 8. Engine `Mixer` is N-channel; pad/channel counts are config (default 16 pads / N active channels). |
| **Colour picker** | Borrow waaaves' `key_color` UX; promote to engine as a reusable **egui** widget in `rustjay-gui/egui_widgets.rs` (§4b). |
| **Tempo source** | Use the **engine tempo** (`rustjay-sync`: tap / Ableton Link / Pro DJ Link → `beat_phase`). Sync ratios = the LFO **beat divisions** (`rustjay-core::lfo::BEAT_DIVISIONS`). No second clock (§4a). |

## 2. Reuse vs. Port

The big win: most of 404's bulk is already engine infrastructure. Only the
sampler domain + HAP codec are genuinely new.

| 404 area | LOC | Action | Engine home |
|---|--:|---|---|
| audio (fft/device/routing) | 962 | **reuse** | `rustjay-audio` |
| input (midi/osc/keyboard/router) | 1285 | **reuse** | `rustjay-control` |
| output (ndi/v4l2/spout senders) | 852 | **reuse** | `rustjay-io` `OutputManager` |
| video/capture + interapp (webcam/syphon/spout/v4l2 in) | ~1600 | **reuse** | `rustjay-io` input sources |
| engine/mixer (blend modes, compositing) | 1177 | **reuse** | `rustjay-mixer` (16 modes ⊇ 404's 12) |
| app/state/config, window, presets, GUI host | 1820 | **reuse** | `rustjay-engine` plugin lifecycle |
| **HAP decode + player + encode** | ~4100 | **reuse / upstream** | `hap-wgpu` (owned) — `HapPlayer`, `HapVideoEncoder`, `GpuDxtCompressor` already exist |
| **HAP YCoCg/dual-plane-alpha convert** | (shader) | **upstream** | hap-wgpu decode-side convert, ref varda-orig `hap_convert.wgsl` |
| **Sampler: Sample/Pad/Bank/Library/Thumbnail** | 936 | **port → example** | `examples/vp404` |
| **Pad sequencer: clock/pattern/step/track** | 1374 | **port → example** | `examples/vp404` |
| **Chroma/Luma keying** | (in mixer) | **extend** | `rustjay-mixer` per-channel key params |
| **Pad-grid + sequencer + browser UI** | ~3200 imgui | **rebuild → egui** | `examples/vp404` egui tabs |

**Domain stays in the example.** Sample/Pad/Bank/sequencer are app types — they
do **not** leak into shared crates (skill pitfall: "leaking app domain types into
shared crates"). The example owns its schema via `EngineState::app_state` +
`param_resolver`.

## 3. HAP dependency: `hap-wgpu` (owned — no wrapper crate)

`hap-wgpu` lives at `/Users/ac/developer/rust/hap-rs/` (3-crate workspace:
`hap-parser` BCn+Snappy, `hap-qt` QuickTime container random-access, `hap-wgpu`
GPU). All published 0.1.0, all already migrated to wgpu 29 (git log:
`feat: migrate hap-wgpu to wgpu 29`). **A `rustjay-hap` wrapper would be pure
ceremony over an API we own and that already does the job.** Depend on `hap-wgpu`
directly; when something's missing, add it *to hap-wgpu* and publish.

What hap-wgpu already gives us (verified in `hap-wgpu/src/lib.rs`):

- **`HapPlayer`** — `open(path)`, `play/pause/stop`, `set_speed`, `set_loop_mode`
  (`LoopMode`), `seek_to_frame`, `update() -> Option<Arc<HapTexture>>`,
  `dimensions/frame_count/fps/duration/playback_state`. A complete clip player.
- **`HapTexture`** — `{ texture: Arc<wgpu::Texture>, view, frame_index, format }`.
  Holds the **raw BCn block texture** (no CPU decompress — the speed win).
- **`HapVideoEncoder`** + **`GpuDxtCompressor`** — GPU DXT/BC3/YCoCg-BC3 compress,
  `encode_from_images/_frames`, `EncodeConfig`, quality presets, co64 (>4GB).
  The live-sampling encoder is **already done** — wire it, don't port 404's.
- `is_supported(adapter)` / `check_support` / `required_features()` for gating.

**The one real gap — decode-side color conversion.** `HapTexture` is a raw BCn
texture whose `format` says how to interpret it. For HapY / HAP Q the RGB channels
hold **YCoCg** (needs a `ycocg_to_rgb` pass) and HAP Q Alpha has a **separate BC4
alpha plane** (dual-plane composite). hap-wgpu only does YCoCg on the *encode*
side. **varda-orig already solved the decode side** in
`src/internal/renderer/shaders/hap_convert.wgsl` (YCoCg→RGB + dual-plane alpha,
with `uv_scale/offset` and `opacity`). Plan:

1. **MVP:** do the convert as a small pass in `examples/vp404` (sample
   `HapTexture` → render to a normal **BGRA8** target → feed the mixer Channel).
   Port `hap_convert.wgsl` as the reference. BGRA8 end-to-end (skill pitfall:
   RGBA + manual swizzle → R/B-swapped NDI/Syphon).
2. **Then upstream** the convert into hap-wgpu (e.g. `HapTexture::to_rgba(device,
   queue) -> wgpu::Texture` or a `HapConvertPipeline`) so every consumer gets
   sample-ready frames, and publish. Keeps the codec concern in the codec crate.

**Offline test (in hap-wgpu, owned):** open a sample `.hap.mov`, assert frame
count + first-frame dimensions per format variant, so a codec regression fails at
`cargo test`, not GPU init.

## 4. Architecture in the engine

```
EffectPlugin (Vp404)  ──render() override──▶ rustjay_mixer::Mixer (root)
  ├─ PadEngine: N SamplePads (count = setting), trigger FSM (Gate/Latch/One-Shot)
  │    each *active* pad → hap_wgpu::HapPlayer.update() → HapTexture (raw BCn)
  │        → HAP convert pass (YCoCg→RGB + dual-plane alpha) → BGRA8  ─┐
  ├─ SequencerEngine: slaved to engine beat_phase → fires triggers     ├─▶ N Mixer Channels
  ├─ keying params (chroma/luma) per Channel  ─────────────────────────┘    (count = setting)
  └─ LiveSampler FSM: capture (rustjay-io input) → hap_wgpu::HapVideoEncoder → new pad
        engine tempo: rustjay-sync (tap / Link / ProDJ) → beat_phase φ → synced pads + sequencer
```

- **Mixer-as-root**: `render()` returns `true` (engine skips its default pass),
  exactly like the `examples/vjarda` stub. Each channel is fed the current HAP
  frame texture of whichever pad is playing into it. Channel/pad counts are a
  setting (don't hard-code 8).
- **Channel output ping-pong**: anything caching a channel/deck `output_texture()`
  must invalidate on its `generation` (skill pitfall). Reuse the engine's mixer,
  don't reimplement compositing.
- **Params are engine-canonical**: pad opacity, blend, speed, key threshold/color
  declared as `ParameterDescriptor`s so MIDI/OSC/HTTP/modulation all reach them.
  Pad *triggers* are events (MIDI note / OSC `/trigger`), routed via
  `rustjay-control`, not params.
- **Persistence**: Banks (16 pads) save/load as JSON via the example's
  `serialize_preset_state`; clips referenced by path, re-resolved on load.

## 4a. Tempo-sync playback (new feature — not in original 404)

Match a clip's loop to the beat, honouring per-pad **in/out points**. Two parts.

**Tempo comes from the engine, not a new clock.** `rustjay-sync` already provides
tap tempo / Ableton Link / Pro DJ Link → a `bpm` + continuous `beat_phase` (`φ`),
the same source the modulation LFOs sync to. **Reuse it.**

**Sync ratio = the LFO beat divisions.** Don't invent a ratio control — reuse
`rustjay_core::lfo::BEAT_DIVISIONS = [0.25,0.5,1,2,4,8,16,32]` /
`BEAT_DIVISION_NAMES = ["1/16","1/8","1/4","1/2","1","2","4","8"]` (cycle length in
beats). Per-pad picks a division → `R = BEAT_DIVISIONS[division]` beats per loop.
Same dropdown the LFO tab uses, so it's instantly familiar.

**Definitions.** in/out frames `in,out`; loop length `L = out - in`; clip `fps`;
engine `bpm`; loop spans `R` beats (from the division pick).

**Rate-match (the math).** Native loop duration `T_native = L / fps`; target
`T_target = R · 60 / bpm`. Required speed:

```
speed = T_native / T_target = (L · bpm) / (fps · R · 60)
```

**Phase-lock (the elegant part).** Rate-match alone keeps the loop *length* right
but its start can sit anywhere on the bar. Instead of free-running at `speed` and
snapping periodically (causes visible jumps when drift accumulates), **derive the
frame directly from the engine `beat_phase`** `φ`:

```
frame = in + fract(φ / R) · L          // seek_to_frame(frame) each render
```

The clip position is now a pure function of `φ` → **beat-locked by construction,
zero drift, loop moment always lands on the beat.** `set_speed` isn't integrated
for synced pads (the formula above *is* the effective rate); show it read-only.

**No hap-wgpu change needed.** 404's `SamplePad` (`src/sampler/pad.rs`) already
owns `current_frame`, resets it to `in_point` on trigger, and advances/wraps it
within `[in_point, out_point]` itself — it **bypasses** hap-wgpu's whole-clip
`update()` loop and uses hap-wgpu purely as a frame decoder (`seek_to_frame` →
`HapTexture`). `Sample` already carries `in_point`/`out_point` (`sample.rs`). Port
that pad layer; hap-wgpu stays codec-only.

**Two per-pad playback modes (same plumbing, different advance rule):**
- **Free** (default SP-404) — port 404's existing `pad.rs` advance:
  `current_frame += speed·fps·dt`, wrap within `[in,out]`.
- **Synced** — replace the advance rule with `current_frame = in + fract(φ/R)·L`.
  Beat-locked by construction; reuse the same `seek_to_frame` plumbing.

**What this needs:** nothing new — `φ`/`bpm` already exist on the engine
(`rustjay-sync`, independent of the VP-404 sequencer). So **full phase-lock can
land in Phase 1**, not Phase 2. The Phase 2 sequencer slaves to the *same*
`beat_phase` (it does not run its own clock — resolves open Q#5).

**Phasing:** port `SamplePad` (in/out + Free advance) + division-based rate-match +
phase-lock all land in **Phase 1**, reading engine `beat_phase`.

**Note:** because `R` is a beat-division pick (powers-of-two-ish), absurd speeds on
very short/long clips are a UX choice, not a math problem — the user picks a
division that suits the clip, exactly like an LFO. No auto-warp needed.

## 4b. Modulation-compatible colour param (borrow from waaaves)

waaaves' `key_color` (`examples/waaaves/src/tabs/mod.rs`) is the picker to borrow —
but its value isn't the swatch (egui already has `color_edit_button_rgb`), it's
that the picker writes **three modulatable params** `key_value_r/g/b` (+ shows LFO
dots), so keying colour stays MIDI/OSC/LFO-controllable. Luma mode collapses to one
value driving all three.

**Don't reinvent egui's picker** — add a thin reusable widget to
`rustjay-gui/src/egui_widgets.rs` (home of `param_slider`) that wraps
`color_edit_button_rgb` + three `param_slider`s bound to `<prefix>_key_value_{r,g,b}`
with the luma/chroma split. That's the engine promotion the user asked for; VP-404's
keying UI (Phase 3) is its first consumer. **Caveat:** waaaves is *imgui*, so this is
an egui re-implementation of the UX, not a code lift.

## 5. Phases (MVP-first)

**Phase 0 — one clip plays.**
Add `hap-wgpu` dep. Minimal `EffectPlugin` that `HapPlayer::open`s a `.hap.mov`,
runs the HAP convert pass (port `hap_convert.wgsl`), and blits the BGRA8 frame to
the output window. Gate: a YCoCg clip loops on screen *with correct colour* (this
proves the convert pass); no-default-features green.

**Phase 1 — Pad grid + triggering + mix + live sampling.**
- **1a/1b DONE:** `Sample`/`Pad`/`Bank` ported; 16-pad grid + `PadGridTab`
  (`AnyEguiTab`); Gate/Latch/One-Shot triggers; command/roster split between UI
  and render thread.
- **1c DONE:** N playing pads composited via `rustjay-mixer` (mixer-as-root in
  `render()` override). Each pad feeds its own `Channel`; channel opacity/blend
  become the pad mix controls. Pad `speed` is exposed as a per-channel engine
  parameter (`ch_pad<N>_speed`) and read in `prepare()` so MIDI/OSC/LFO reach it.
  `PadChannel` implements `EffectInstance` and runs the HAP→RGBA convert pass
  into the channel texture. `rustjay-mixer::MAX_CHANNELS` was raised from 8 to 16
  so all 16 pads can have a channel; the hard cap in `Mixer::add_channel` now
  uses `MAX_CHANNELS`. Verified visually: single pad plays; two different clips
  on pads 0+1 composite correctly through the mixer.
- **1d DONE:** per-pad **in/out points** + Free-mode looping (already in `Pad`)
  + **tempo-sync** (Free/Synced, division-based, reading engine `beat_phase` —
  full §4a, both rate-match and phase-lock).
- **1e DONE:** LiveSampler FSM — capture from a `rustjay-io` input →
  `hap_wgpu::HapVideoEncoder` (`HapFormat::Hap5` only, to avoid the upstream
  YCoCg encoder convention issue) → assign to pad. Gated behind a `capture`
  feature that pulls in `rustjay-io`; default build stays lean.
- Gate: trigger pads from the grid + MIDI; clips composite to output; capture a
  webcam clip to a pad.

**Phase 2 — Pad sequencer DONE.**
Ported pattern/step/track + `SequencerEngine` (polyphonic, gate-release,
probability/ratchet, pattern chaining). Slaved to the engine `beat_phase` via the
same accumulated-beat clock used for tempo-sync pads; 404's standalone
`sequencer/clock.rs` is dropped. `SequencerTab` (egui) with click-to-toggle steps,
play/stop/reset/clear, per-track mutes, and pattern queue/select. Distinct from the
engine's modulation `StepSequencer` (a mod source).

**Phase C — Sequencer step-editing via web + MIDI step-record DONE (2026-06-21).**
`POST /api/app/command` generic route added to `rustjay-api` (opaque JSON body →
`EngineState::app_command_queue`; always present, populated by web layer when `api`
is on). VP-404 drains it in `prepare()` and interprets `SeqCmd` (`ToggleStep`,
`SetStep`, `SetLength`, `SelectPattern`, `SetEditStep`). MIDI step-record: while
sequencer is stopped, any `pad<i>_trig` rising edge writes track `i` as active at
`self.edit_step` (via pure `api_state::step_write`) and advances the cursor, then
still triggers the pad for audition. `pad_grid.html` extended with a full 16-track ×
N-step grid (horizontal-scroll for 32/48/64 steps), pattern/length selectors, edit
cursor indicator, step-toggle via `POST /api/app/command`. 4 new unit tests
(step_write records+advances, cursor wraps, SeqCmd toggle, SeqCmd set-length).
Default build (no `api`): 22 tests. With `api`: 24 tests.

**Phase B — Browser pad grid via rustjay-api DONE (2026-06-21).**
`api` feature added to `vp404/Cargo.toml` (forwards to `rustjay-engine/api`; off by
default). `api_state.rs` (app-owned) defines `Vp404Snapshot`/`PadSnapshot` +
`build_snapshot(bank, seq)`; published into `EngineState::app_state` each frame from
`render()`. `pad_grid.html` (16-button touch grid, pointer events → `PUT
/api/app/params` setting `pad<i>_trig`, live state via polling `GET /api/app/state`
+ WS delta stream). `on_engine_ready()` sets `engine.app_ui_html` (new generic field
on `EngineState` in `rustjay-core`) so `GET /api/app/ui` (new route in
`rustjay-api::build_router`) serves the page. No VP-404 types leaked into shared
crates; schema is vp404-internal. Build clean with and without `api` feature.

**Phase A — Trigger params + expandable sequencer DONE (2026-06-21).**
16 `pad<i>_trig` params (0..1, default 0) registered in `parameters()`. Edge
detector in `prepare()` reads each param via `get_param_base`, fires `pad.trigger()`
on rising edge (>0.5) and `pad.release()` on falling — one trigger path for MIDI,
OSC, web, and the grid UI. Grid-tab buttons now set `pad<i>_trig` via
`engine.set_param_base` instead of posting `PadCmd::Trigger/Release` directly.
`SequencerTab` now renders `pattern.length()` steps (wrapped into rows of 16) and
adds a length combo (16/32/48/64) that calls `Pattern::set_length`. 3 edge-detector
unit tests added.

**Phase 3 — Keying. DONE.**
Per-channel chroma/luma key added to `rustjay-mixer`: `key_mode` (None/Chroma/Luma),
`key_r/g/b` (LFO-modulatable colour), `key_threshold`, `key_smoothness`,
`key_luma_invert` as engine params per channel (prefix `ch_{uuid}_key_*`).
`composite.wgsl` extended (32→64 bytes) with keying logic that modifies source alpha
before blending — all blend modes inherit key masking automatically. `KeyParams`
re-exported from `rustjay-mixer`. Reusable `key_color_picker` widget added to
`rustjay-gui/egui_widgets.rs` (swatch + R/G/B param sliders for MIDI/OSC/LFO
control) and re-exported through the engine prelude. VP-404 grid tab has a "Key
mode" combo + chroma/luma controls per selected pad. 24 mixer tests + 17 vp404 tests
pass; `composite_shader_validates` covers the new WGSL.

**Phase 4 — Browser/library + polish. DONE (thumbnails deferred).**
`BrowserTab` (`browser_tab.rs`): scans a directory for `.mov/.mp4/.m4v/.avi` video
files; `.hap.mov` files show a "Load" button (direct); non-HAP files show "Convert &
Load" which runs `ffmpeg -c:v hap -format hap_q -chunks 4` in a background thread,
auto-loads on completion, and updates the entry to HAP so the next click is direct.
Initial dir = the launch clip's parent directory. `SamplerTab` (`sampler_tab.rs`,
`--features capture`): dedicated tab with target pad + duration slider (1–30 s,
→ frame count at 30 fps) + Record/Cancel + spinner; replaced the buried capture
buttons in `PadGridTab`. Pad thumbnails deferred (low priority per user).

**Phase 5 — Output senders.**
Wire `rustjay-io` `OutputManager` for NDI/Syphon/Spout/V4L2 out + recording. Reuse
the top-bar output pills pattern from vjarda; no new code in shared crates.

**Phase 6 — Control QoL (2026-06-22).**
- **Draggable multi-step gates.** The sequencer step grid is a painted lane:
  click toggles a step (default 1-step gate); drag a step's gate rightward to tie
  it across cells. `Step.gate_length` is now measured in **steps** (>1 = tied);
  the engine's upper clamp is removed. Tails crossing a 16-step sub-row boundary
  fire correctly but aren't drawn.
- **SP-404 start/end trim.** Global mappable `in_point` / `out_point` knobs
  retrim the **last-pressed** pad's `Sample` range, applied only when a knob
  actually moves so idle pads keep their own trim. On-screen slider doesn't
  follow selection (fine for endless encoders) because `prepare()` only gets
  `&EngineState`.
- **MIDI note triggers.** `pad{i}_trig` descriptors make pads MIDI-learnable; a
  learned **Note** now acts as a button — Note-On drives the param to **max**
  (ignoring velocity so soft pad hits still fire), Note-Off to min.
- These ride on engine-wide control upgrades (shared, benefit every example):
  top-bar **MIDI MAP / LFO MAP** modes (`map_mode_active` +
  `apply_param_map_overlay`), a **MIDI monitor** (`EngineState.midi_last_input`),
  re-learn **overwrites** instead of stacking, and saved device+mappings now
  **reconnect and restore** on launch in the winit path.

## 6. Open questions / risks (decide at implementation)

1. **Keying placement** — new `BlendMode::ChromaKey/LumaKey` variants vs. a
   separate per-channel key stage that runs before blend. 404 models them as
   MixModes, but they need extra params (color/threshold/smoothness) a plain
   blend mode doesn't carry. Lean: **per-channel key params + a key pass**, blend
   stays orthogonal.
2. **HAP convert coverage** — confirm the convert pass handles every variant in
   the user's `samples/` (Hap1/Hap5 = direct RGBA; HapY/Q = YCoCg; HapA/Q-Alpha =
   dual-plane; BC7/BC6H = direct). varda-orig's shader covers YCoCg + dual-plane;
   check Hap7/HapH (BC7/BC6H) sample natively without conversion.
3. **syphon-wgpu skew** — 404 pins `syphon-wgpu 0.1.1`, engine workspace uses
   `0.2.0`. Use the engine's `rustjay-io` Syphon path; don't pull 404's pin.

**Resolved (2026-06-20):** ~~channel count~~ → setting (not fixed 8). ~~clock
ownership~~ → engine `beat_phase` is the single master; sequencer + synced pads both
slave to it, 404's `sequencer/clock.rs` is dropped. ~~tempo ratio~~ → reuse LFO
`BEAT_DIVISIONS`. ~~name~~ → VP-404 / `examples/vp404`. ~~colour picker~~ → reusable
egui widget in `rustjay-gui/egui_widgets.rs` (§4b). ~~mixer ownership~~ → `Mixer` is
not `Sync` (it holds `RefCell`s and `Box<dyn EffectInstance>`), so the plugin keeps it
in an `Arc<Mutex<Mixer>>` like `examples/vjarda`; `parameters()` and `render()` lock it.

## 7. Build & verify discipline (per skill)

- No-default-features build green; `encode`, `ndi`, Syphon all feature-gated.
- Per-app `app_name()` = `"VP-404"` → isolated config/preset file.
- Copy an existing example's `build.rs` (Syphon `-rpath` link-arg doesn't
  propagate to leaf binaries on macOS).
- Verify visually by running (`/run`, `/verify`), not by inspection.
- Update `guide/` when public API changes; pin `ffmpeg-next` default-features off
  if any decode fallback is added.
