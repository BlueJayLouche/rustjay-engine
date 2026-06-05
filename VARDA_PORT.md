# Varda Port Roadmap

Porting the full **Varda** broadcast-routing VJ application onto **rustjay-engine**,
assembled as `examples/varda`.

> Architect planning document. Companion to [`WAAAVES_PORT.md`](WAAAVES_PORT.md)
> (the proven "port an app into the engine as an example" playbook) and the
> `rustjay` agent skill at [`.agents/skills/rustjay/`](.agents/skills/rustjay/SKILL.md).

---

## 0. Current status (2026-06-05)

Live parity detail lives in [`examples/varda/PARITY.md`](examples/varda/PARITY.md);
this is the phase-level rollup.

**Phases 0–7 reviewed and verified (2026-06-05).** Phase 5 complete:
T05.1/05.2 — MIDI and OSC reach the canonical engine params (OSC via the
auto-registered OSC address on each `param_descriptor`; MIDI via learned maps).
T05.4 — `param_router` maps the hierarchical `deck|channel/<uuid>/param/<name>`
namespace **structurally** to flat canonical ids, wired into the engine's
`WebCommand::Set` and MIDI param-path fallback via `EngineState::param_resolver`.
T05.3 — HTTP/WS via **generic, app-agnostic** routes on `rustjay-api` (behind the
`api` feature): `GET /api/app/state` serves the opaque JSON snapshot the app
publishes into `EngineState::app_state`; `GET|PUT /api/app/params` lists/sets
params, writes resolving through the same `param_resolver` → `WebCommand::Set`.
The **Varda schema lives entirely in `examples/varda`** (`api_state.rs`), rebuilt
with live values every frame into `app_state`; that slot is included in the WS
snapshot so JSON-Patch deltas carry app state on change. (The shared crate knows
no Varda types — a review-rework fix; the prior draft had hard-coded Varda DTOs
+ `/api/varda/*` routes into rustjay-api.) Phase 6: T06.1–T06.9 custom egui tabs
(Mixer, Deck, Effects/Library, Modulation, MIDI, + Stage/Outputs/Sequencer/Inspector
stubs) wired via `run_with_egui_tabs`. **Each tab is non-replacing and gets its own
sidebar button** (Mixer/Deck/Effects drive live params through canonical engine ids:
`crossfader`, `ch_<uuid>_opacity/blend`, `ch_<uuid>_deck_<uuid>_opacity/blend`, FX
enable toggles); Modulation/MIDI are read-only info panels and the built-in
LFO/MIDI/Input tabs are **kept** (no `hidden_tabs()`), so no working UI is lost.
This required an **engine-host fix**: the egui host (`rustjay-gui`) now renders a
sidebar button per non-replacing custom tab (previously only `replaces()` tabs were
reachable — they were dead because their replaced builtins were hidden). Per-item
`ui.push_id(uuid)` scopes and blocking (poison-tolerant) mixer locks added.
Phase 7: T07.1–T07.3 surface model (`VardaSurface`/`VardaStage`), corner-pin/mesh
warp, 2D StageTab with SVG/DXF contour import, and the combined
`run_with_projection_egui_tabs` entrypoint in `rustjay-engine` (egui tabs + projection
outputs simultaneously). **Warp actually reaches the projector:** the StageTab's
corner-pin/mesh edits to the Master surface flow through a shared `WarpSync`
(`Arc<Mutex>`, plugin-owned, injected into app state) into a `VardaWarpStage`
(`rustjay_projection::WarpStage::from_mode` + live `set_homography`) in the projector
stage chain — corner drags update the homography in place each change; a mode/mesh
switch rebuilds. Per-surface source selector (Master/Channel/Deck/Domemaster) is
modeled in data + UI but only **Master** renders to the projector, and the properties
panel edits surface 0 only — non-Master source routing, per-surface selection, and
multi-surface output are Phase 8 follow-ups.
*(Live GUI/projection smoke-test — actually running and drawing a warped surface —
still pending; needs a display.)* Current gate: `cargo build -p varda`
(default / `--no-default-features` / `--all-features` / `-F api` / `-F projection`)
green and warning-clean, `cargo clippy -p varda -p rustjay-gui` clean, `cargo test`
`-p rustjay-mixer` (21) / `-p varda` (5) / `-p rustjay-api` (9) / `-p rustjay-core`
(83) green, `cargo build --workspace` green.

| Phase | State | Notes |
|-------|-------|-------|
| **0 — Scaffolding & parity harness** | ✅ done (T00.4 golden-image harness deferred) | Module tree, feature flags, parity tracker, rustjay-io coverage probe all in place. Golden-image diff harness (T00.4) not yet wired. |
| **1 — Routing graph core** | ✅ done | Deck → DeckCompositor → `rustjay_mixer::Channel` → Mixer spine runs. Zero-opacity culling **verified to skip the GPU pass** (`compositor.rs`, not multiply-by-zero). Deck opacity/blend now read through `engine.get_param` (post-review fix). |
| **2 — Sources** | ✅ done | ISF (rustjay-isf + `EffectNode`), camera (shared `InputManager`), image (PNG/JPG scan), solid color, registry (ISF + image + video stubs), `notify` watcher + hot-reload all wired. Video/HAP/SRT/HLS/DASH/RTMP remain absent (rustjay-io gap — port required, see PARITY probe). |
| **3 — Effect chains** | ✅ done | 3-level hierarchy (deck / channel / master) with `add_effect` + `set_effect_enabled`; stable FX UUID prefixes (`fx<uuid>_`); `reorder_fx`/`move_fx` APIs on `Deck`; per-effect enable honored in all render paths; params reachable via canonical prefixes. GUI wiring and demo assembly FX exercise are follow-ups. |
| **4 — Modulation** | ✅ done | Mixer `ModulationEngine` wired to crossfader, channel opacities, and deck opacities. `Arc<Mutex<ModulationEngine>>` shared with `DeckCompositor` so mixer-level modulation reaches deck-level params. Demo: LFO on crossfader + deck opacities; audio-band (bass) on crossfader. Engine `AudioState` → `AudioValues` bridge feeds FFT into `ModulationEngine::update`. ADSR + step-sequencer + mod-on-mod are engine-present but not yet demoed. |
| **5 — Control** | ✅ done | T05.1/05.2 MIDI/OSC reach canonical params; T05.4 param_router wired into `WebCommand::Set` + MIDI fallback; T05.3 **generic** `rustjay-api` routes (`GET /api/app/state`, `GET\|PUT /api/app/params`) — app publishes its schema into the opaque `EngineState::app_state`; WS JSON-Patch deltas carry it. |
| **6 — GUI** | ✅ done *(live click-test pending)* | Non-replacing egui tabs each with own sidebar button (engine-host fix in `rustjay-gui`). Mixer/Deck/Effects drive live graph params via canonical ids; Modulation/MIDI are read-only panels (built-in LFO/MIDI retained, nothing hidden). Stage/Outputs/Sequencer/Inspector stubbed (Phase 7+/11/12). |
| **7 — Surfaces & projection** | ✅ done *(Master corner-pin/mesh warp reaches projector; live display smoke-test pending)* | T07.1 surface model (polygon/circle + source enum); T07.2 corner-pin/mesh warp **wired to projector output** via `VardaWarpStage`+`WarpSync` bridge (Master surface); T07.3 StageTab 2D canvas + surface list + warp editor + SVG/DXF import. Combined `run_with_projection_egui_tabs` entrypoint in `rustjay-engine`. Non-Master source routing, per-surface selection, multi-surface output → Phase 8. |
| **8–14** | ⬜ not started | Multi-output, streaming, recording, persistence, transitions, dome/edge-blend, parity audit. |

### Carry-over backlog (deferred items from "done" phases — clear opportunistically)

- **T00.4** Golden-image harness for headless render diffs (Phase-0 acceptance gate, deferred).
- **T04.3 / T04.4** ADSR envelope, step-sequencer, and mod-on-mod chaining are present in the engine/mixer modulation but **not yet demoed or wired into the Varda graph** — the Phase-4 demo only exercises LFO + audio-band on crossfader/deck opacity.
- **FX demo exercise** — deck & channel FX chains have working `add_effect`/`set_effect_enabled`/`reorder_fx` APIs but are **not exercised** in the demo assembly; add a deck/channel FX to prove the path end-to-end (blocked on, or pairs with, Phase 6 GUI).
- **Camera mixed-resolution** — the shared `CameraSession` hardcodes 1280×720 until the first frame; two decks requesting the *same* device at *different* sizes can mismatch the upload stride. Fine for single-size use; revisit if multi-resolution camera decks appear.

### Conventions established (do not regress)

- **Parameter scheme:** every level's `parameters()` returns ids prefixed with **only its own component**; the enclosing level adds its prefix (`Mixer` adds `ch_<uuid>_`, `DeckCompositor` decks add `deck_<uuid>_`). At render, read modulated values through `engine.get_param(&cached_key)` — **never** from local struct fields. `Deck` mirrors `rustjay_mixer::Channel` (cached `opacity_key`/`blend_key`, `set_full_prefix` propagation). This is the fix for the "modulation does nothing" class of bug; keep new nodes consistent.
- **Perf:** no per-frame allocations in `render`/`prepare`/`build_uniforms` (reuse scratch buffers, cache key strings); zero-opacity layers must skip the pass.
- **Features:** heavy features (`ndi`/`api`/`projection`/streaming/recording/syphon) stay off by default; the `--no-default-features` build stays green and warning-clean.
- **Modulation single-authority:** effective opacity/crossfader = `engine.get_param` base **plus** the mixer's `ModulationEngine` (this is `rustjay_mixer`'s established pattern, now extended to deck opacity via a shared `Arc<Mutex<ModulationEngine>>`). A given param key must be assigned in **exactly one** modulation system — assign through the mixer's `ModulationEngine` (as the app does), never *also* the engine's `LfoBank`, or the two contributions double-sum.
- **Control targets canonical ids:** all external control (HTTP/MIDI/OSC) ultimately writes a flat canonical engine id (`crossfader`, `ch_<uuid>_…`, `ch_<uuid>_deck_<uuid>_…`) via `engine.set_param_base`. Hierarchical addresses are translated by `examples/varda`'s `ParamRouter` (via `EngineState::param_resolver`) — a thin string mapping, **not** a second param store. New control surfaces must resolve to these same ids; never read/write control state off to the side.

---

## 1. Decisions (locked)

| Axis | Decision | Consequence |
|------|----------|-------------|
| **Fidelity** | **Full parity** in one roadmap | Every Varda subsystem is planned: routing graph, 3-level FX, modulation, MIDI/OSC/HTTP, projection mapping, multi-output, NDI/SRT/HLS/DASH, recording, dome, edge-blend, persistence. |
| **Code placement** | **Port Varda internals into `examples/varda`**, reuse engine crates where convenient | The app, routing model, scene/stage, and GUI live in `examples/varda`. Heavy GPU/codec/protocol machinery is delegated to existing crates (`rustjay-render`, `-mixer`, `-projection`, `-isf`, `-io`, `-control`, `-audio`, `-api`). |
| **GUI** | **Engine egui tab system** (`run_with_egui_tabs` + `AnyEguiTab`) | Varda's 11 egui panels are rebuilt as `AnyEguiTab` implementors over `engine.get_param/set_param_base` and `param_slider*` helpers, consistent with `examples/delta-egui`. |
| **Skill** | **Broad `rustjay` skill + worked Varda case study** | `.agents/skills/rustjay/SKILL.md` plus `references/`, including `varda-assembly-case-study.md` that doubles as living documentation of this port. |

---

## 2. The core architectural shift

Varda is a **monolithic binary** (`fn main` owns the window, the render loop, the
engine, the UI). rustjay-engine **inverts control**: you implement the
`EffectPlugin` trait and hand it to `rustjay_engine::run*(...)`, which owns the
window, surface, swapchain, audio thread, control servers, GUI, presets, and the
frame loop. The app is a *plugin*, not a *main*.

```
        Varda (today)                    Varda-on-engine (target)
   ┌───────────────────────┐        ┌──────────────────────────────────┐
   │ fn main               │        │ rustjay_engine::run_with_egui_tabs│
   │  ├ winit + wgpu       │        │   owns: window, wgpu, swapchain,  │
   │  ├ audio thread       │        │   audio, MIDI/OSC/web servers,    │
   │  ├ midi/osc/http      │        │   presets, GUI host, frame loop   │
   │  ├ egui panels        │        │            │                      │
   │  ├ render loop        │        │            ▼                      │
   │  └ scene/stage state  │        │   VardaPlugin: EffectPlugin       │
   └───────────────────────┘        │     ├ render(): drives routing    │
                                     │     │   graph → mixer → surfaces  │
                                     │     ├ parameters(): exposes paths │
                                     │     └ State: scene + stage        │
                                     │   tabs: Vec<Box<dyn AnyEguiTab>>  │
                                     └──────────────────────────────────┘
```

The `examples/varda` **stub already proves the spine**: it wraps `rustjay-mixer`
as the engine root via `EffectPlugin::render`, loads ISF channels through
`rustjay_isf::IsfEffect` + `rustjay_render::EffectNode`, and exposes mixer
parameters through `EffectPlugin::parameters`. Everything below extends that spine.

### Extension points the port will use

`EffectPlugin` (from `rustjay-core/src/plugin.rs`) — the methods that matter here:

- `type State` (serde) — **holds the entire Varda scene + stage**.
- `type Uniforms` — root uniforms (the routing graph mostly renders via sub-nodes).
- `init(device, queue)` — build GPU resources, load sources, construct the graph.
- `parameters()` / `parameters_dirty()` / `clear_parameters_dirty()` — expose the
  full modulatable parameter set (`deck/<uuid>/param/<name>`, `crossfader`,
  `ch/<uuid>/opacity`, …) so MIDI/OSC/LFO/HTTP can all target it.
- `prepare()` — per-frame CPU update (tick modulation, transitions, decode pulls).
- `render(RenderHookCtx, &mut State)` — the routing-graph render: decks → deck FX →
  channels → channel FX → mixer → master FX → surfaces → outputs.
- `render_graph()` / `mesh_descriptor()` / `compute_shader()` — available if a
  subsystem wants declarative multi-pass instead of an imperative hook.
- `serialize_preset_state` / `deserialize_preset_state` / `on_preset_applied` —
  deck/channel preset bridge into the engine's preset bank.
- `hidden_tabs()` — suppress built-in tabs that Varda replaces.

Entrypoints (`rustjay-engine/src/lib.rs`): `run`, `run_with_tabs`,
`run_with_egui_tabs`, `run_with_projection`, `run_headless*`, `run_*_gles2_*`.
The port targets **`run_with_egui_tabs`** for the desktop build and
**`run_with_projection` / `run_headless*`** for projection-mapped and headless
outputs.

---

## 3. Subsystem mapping: reuse vs. port

For each Varda `src/internal` module (LOC measured), the disposition. **Reuse** =
delegate to an engine crate. **Port** = bring the module into `examples/varda`,
adapting to the plugin model. **Extend** = the crate covers most of it; add the gap.

| Varda module | LOC | Disposition | Engine target / notes |
|--------------|-----|-------------|------------------------|
| `renderer` | 7644 | **Reuse-heavy** | `rustjay-render` (`WgpuEngine`, `EffectNode`, `Texture`, `PreviousFrameTexture`) + `rustjay-mixer` compositing + `rustjay-projection` warp. This is the biggest *reuse* win — most of it is engine-owned already. |
| `surface` | 2682 | **Extend** | `rustjay-projection` (`stage.rs`, `surface_import.rs`, `warp.rs`, `edge_blend.rs`, `auto_blend.rs`, `slicer.rs`). Port surface→source routing model on top. |
| `deck` | 2554 | **Port** | No engine equivalent for "source + FX chain + opacity + blend + scaling" as a unit. New `varda::graph::Deck`. Sources delegate to `-io`/`-isf`. |
| `midi` | 2395 | **Reuse** | `rustjay-control/midi` (incl. APC-mini profile, auto-map, learn). Bridge param-path mapping. |
| `modulation` | 1604 | **Extend** | `rustjay-core` LFO (`LfoBank`, `Waveform`, `LfoTarget`, beat divisions) + engine audio routing. **Gap:** ADSR, step sequencer, 4-deep mod-on-mod chaining → port the missing sources, route through core targets. |
| `mixer` | 1544 | **Reuse** | `rustjay-mixer` (`Mixer`, `Channel`, `BlendMode`, `AutoCrossfade`, `BeatSyncCrossfade`, `InputSelect`, `tick_transitions`). |
| `channel` | 1537 | **Extend** | `rustjay-mixer::Channel` covers compositing + FX; port the deck-list ownership + per-channel sub-mix routing. |
| `persistence` | 1475 | **Port** | `.varda/` layout (`scene.json`, `stage.json`, `midi.json`, `keymap.json`, `presets/`). Engine has preset bank + config; scene/stage are app-specific. |
| `video` | 1280 | **Extend** | `rustjay-io/input` for camera/decode. **Gap check:** HAP GPU-native + ffmpeg decode path — confirm coverage in `-io`, else port decoder. |
| `scene` | 1122 | **Port** | The scene graph (channels/decks/effects/modulation/crossfader/sequences) = `VardaPlugin::State`. |
| `ndi` | 996 | **Reuse** | `rustjay-io/ndi_runtime` (+ engine NDI output command). Feature-gated. |
| `audio` | 853 | **Reuse** | `rustjay-audio` (2048-bin FFT, bands, beat/BPM) — engine owns the audio thread. |
| `isf` | 818 | **Reuse** | `rustjay-isf` (`IsfEffect::from_path`) + vendored `isf` parser crate + hot-reload. |
| `params` | 784 | **Extend** | `rustjay-core` `ParameterDescriptor`/`ParamCategory`/`ParamType`. Port the hierarchical path scheme (`deck/<uuid>/param/<name>`). |
| `stream` | 677 | **Extend** | `rustjay-io/output` + input. SRT/HLS/DASH/RTMP send-receive — confirm `-io` coverage; port protocol glue gaps. |
| `clock` | 640 | **Extend** | Engine Ableton Link (`LinkState`/`LinkCommand`) + ProDJ + beat phase. Port any transport not covered. |
| `keymap` | 634 | **Port** | Keyboard bindings (`keymap.json`). App-level; engine has no keymap layer. |
| `osc` | 497 | **Reuse** | `rustjay-control/osc`. |
| `registry` | 458 | **Port** | Source/effect registry (library). Drives the Library panel + API. |
| `camera` | 459 | **Reuse** | `rustjay-io/input` (nokhwa) + `v4l2_devices`. |
| `notifications` | 299 | **Port** | Small toast/notification layer for the GUI. Engine has `notifications` module — **reuse if present**, else port. |
| `syphon` | 165 | **Reuse** | Engine syphon output command + `rustjay-io`. macOS only. |
| `sysmon` | 120 | **Port** | CPU/GPU/mem readout for status bar (`sysinfo`). Trivial port. |
| `param_router` | (in params) | **Extend** | Routes incoming control → parameter paths. Bridge to engine `set_param_base`. |
| `recording` | 5 | **Port (greenfield)** | Varda's recorder is a stub; build over `rustjay-io/output` + ffmpeg encode. Lowest existing fidelity. |

**Usecases layer:**

| Varda usecases | LOC | Disposition |
|----------------|-----|-------------|
| `ui/` (11 panels) | 12019 | **Rebuild** as `AnyEguiTab` tabs (see §5). |
| `api/` (16 route groups) | 5770 | **Extend `rustjay-api`** — it already mirrors `system/audio/midi/osc/presets/mixer/modulation/input/output/link/prodj`. Add `decks/channels/scene/surfaces/stage/sequences/library/effects` route groups + WS JSON-Patch deltas. |

---

## 4. Phased task breakdown (full parity)

Phases are ordered so each ends with something runnable. IDs follow the waaaves
`Txx` convention. Each task notes **reuse/port/extend** and an acceptance check.

### Phase 0 — Scaffolding & parity harness ✅ *(T00.4 deferred)*
- **T00.1** Promote `examples/varda` from stub to module tree: `graph/`, `scene/`,
  `stage/`, `sources/`, `ui/`, `persistence/`, `control/`. Keep the current
  mixer-as-root render spine working at every step.
- **T00.2** Feature flags in `examples/varda/Cargo.toml` mirroring engine features
  (`mixer`, `api`, `projection`, `ndi`, `syphon`, `streaming`, `recording`).
- **T00.3** Parity tracker: a checklist mapping each Varda README capability → task
  ID → status. Acceptance gate for "full parity."
- **T00.4** Golden-image harness: reuse `rustjay-projection/test_harness.rs` pattern
  for headless render diffs against reference frames.

### Phase 1 — Routing graph core ✅ *(runnable: multi-deck → channel → mixer)*
- **T01.1 [Port]** `graph::Deck` — source handle + opacity + `BlendMode` + scaling
  + FX chain slot. Renders to an offscreen `Texture`.
- **T01.2 [Extend]** `graph::Channel` wraps `rustjay_mixer::Channel`; owns an ordered
  deck list; composites decks (opacity-culled, zero-opacity skipped) before its FX.
- **T01.3 [Reuse]** Mixer stage = `rustjay_mixer::Mixer` (crossfader for 2,
  per-channel opacity for 3+, master FX chain). Wire `effective_opacities`.
- **T01.4 [Port]** `VardaPlugin::render` drives the whole graph each frame into the
  engine target; `State = Scene`.
- *Acceptance:* 2 channels × 2 ISF decks each, crossfader + per-deck opacity, all
  6 blend modes, zero-opacity culling verified by golden image.

### Phase 2 — Sources 🟡 *(runnable: every source type on a deck)*
- **T02.1 [Reuse]** ISF generators/filters via `rustjay-isf` + hot-reload (`notify`).
- **T02.2 [Extend]** Video decode (ffmpeg) + loop/ping-pong/one-shot, speed, scrub,
  in/out — via `rustjay-io/input`. **Confirm/port HAP** GPU-native path (BC/YCoCg).
- **T02.3 [Reuse]** Camera (nokhwa, shared across decks), Image (PNG/JPG), Solid color.
- **T02.4 [Port]** `registry` — source/effect library that the GUI + API enumerate.
- *Acceptance:* each source type renders on a deck; HAP plays GPU-native; camera
  shared by 2 decks without double-open.

### Phase 3 — Effect chains 🟡
- **T03.1 [Extend]** 3-level FX (deck / channel / master) as ordered `EffectNode`
  lists; reorder + per-effect enable. Master FX via `Mixer::add_master_effect`.
  *(Done: add + per-effect enable at all 3 levels, canonical param prefixes.
  Pending: T03.1b reorder with stable FX ids; demo/GUI exercising deck & channel FX.)*
- **T03.2 [Reuse]** ISF filters as effects; typed params surfaced into the graph.
- *Acceptance:* reorder/toggle an FX at each level; params modulatable.

### Phase 4 — Modulation
- **T04.1 [Reuse]** LFO bank (`rustjay-core` `LfoBank`, 6 waveforms, beat-synced
  divisions) targeting parameter paths.
- **T04.2 [Reuse]** Audio-band routing (bass/mid/treble) via engine audio.
- **T04.3 [Port]** ADSR envelope + Step sequencer sources (engine gap).
- **T04.4 [Port]** Mod-on-mod chaining up to 4 deep; summed targets.
- *Acceptance:* LFO→param, audio→param, ADSR (MIDI-triggered), step-seq, and an
  LFO modulating another LFO's frequency all observable on one parameter.

### Phase 5 — Control (co-equal consumers) ✅ *(live HTTP/WS smoke-test pending)*
- **T05.1 [Reuse]** ✅ MIDI via `rustjay-control/midi` (learn, unlearn, APC-mini,
  auto-map). Bridge to Varda param paths.
- **T05.2 [Reuse]** ✅ OSC via `rustjay-control/osc` (auto-registered addresses).
- **T05.3 [Extend]** ✅ HTTP/WS via `rustjay-api` as **generic, app-agnostic** routes:
  `GET /api/app/state` (opaque app snapshot) + `GET|PUT /api/app/params`, single
  listener, WS JSON-Patch deltas carry `app_state`. The Varda schema lives in
  `examples/varda/api_state.rs` (app-owned), not the shared crate. *(Typed per-
  resource routes — decks/channels/effects/library — are reconstructable client-
  side from the snapshot; not added to the shared crate.)*
- **T05.4 [Extend]** ✅ `param_router` bridging incoming control → `set_param_base`
  via `EngineState::param_resolver` (structural, all params, cross-checked).
- *Acceptance:* same parameter driven identically from MIDI, OSC, and HTTP; Swagger
  UI lists the routes; WS pushes deltas. *(MIDI/OSC/HTTP write paths met in code +
  unit tests; live server smoke-test still to run.)*

### Phase 6 — GUI (egui tabs) — see §5 ✅ done
- **T06.1 MixerTab** — crossfader, per-channel opacity/blend sliders, master FX list.
- **T06.2 DeckTab** — per-channel deck opacity/blend sliders, deck FX enable toggles.
- **T06.3 EffectsTab** — library registry listing + live FX chain enable toggles (deck/channel/master).
- **T06.4 ModulationTab** — modulation sources + assignments read-out; replaces built-in Lfo.
- **T06.5 MidiTab** — replaces built-in Midi tab (MIDI is engine-managed).
- **T06.6–T06.9 Stage/Outputs/Sequencer/Inspector** — stubbed placeholders for future phases.
- *Acceptance:* full live control of the graph from the desktop UI; built-in tabs
  Varda supersedes are hidden via `hidden_tabs()`/`replaces()`.

### Phase 7 — Surfaces & projection mapping ✅ done
- **T07.1 [Extend]** `VardaSurface` / `VardaStage` model in `stage/mod.rs` — polygon
  (vertices) + circle (center/radius) shapes; `SurfaceSource` enum (Master /
  Channel / Deck / Domemaster); stored in `VardaAppState`; default full-frame
  surface created on init.
- **T07.2 [Reuse]** `WarpMode::CornerPin` + `WarpMode::Mesh` per surface. A
  `VardaWarpStage` (in `stage/mod.rs`) sits in the projector stage chain and reads a
  shared `WarpSync` (`Arc<Mutex>`, plugin-owned, injected into `VardaAppState.stage`)
  each frame: it `WarpStage::from_mode`s the initial mode and applies StageTab edits
  via `set_homography` (corner-pin) or rebuild (mesh), version-gated so it re-applies
  only on change. The StageTab `publish_warp`es the Master surface's warp on edit, so
  corner drags actually warp the projector output. Corner-pin editable numerically.
- **T07.3 [Extend]** `StageTab` 2D canvas (egui painter) draws surfaces as
  polygons/circles with labels; left panel lists surfaces + add/remove/import
  buttons; right panel shows source selector, warp mode combo, corner-pin
  drag-values. SVG/DXF/raster contour import via
  `rustjay-projection::surface_import::detect_from_file` → `DetectedContour::to_surface`.
- **Entrypoint** `run_with_projection_egui_tabs` added to `rustjay-engine`
  (`crates/rustjay-engine/src/lib.rs` + `app/mod.rs`) — combines egui tabs
  (`App::new_with_egui`) with projection setup closure, gated on
  `projection` + `egui` features. Varda `main.rs` branches:
  `projection` on → `run_with_projection_egui_tabs` with identity projector;
  `projection` off → `run_with_egui_tabs` (Phase 6 spine preserved).
- *Acceptance:* draw surfaces ✅, assign sources ✅ (data model + UI; only Master
  wired to projector output — Channel/Deck source rendering is Phase 8 follow-up),
  corner-pin warp ✅, import a contour ✅.

### Phase 8 — Multi-output & headless
- **T08.1 [Extend]** Output model: window/fullscreen-on-display + per-surface
  assignment + per-output warp/blend, over engine outputs.
- **T08.2 [Reuse]** Headless outputs via `run_headless*` + `projection/headless.rs`
  async readback (already hardened — see recent commits).
- *Acceptance:* two windows on two displays, distinct surface sets; one headless
  output reading back frames.

### Phase 9 — Streaming I/O *(feature-gated)*
- **T09.1 [Reuse]** NDI send/receive (`rustjay-io/ndi_runtime`).
- **T09.2 [Extend]** SRT / HLS / LL-HLS / DASH / RTMP(S) send+receive (`-io`); port
  protocol glue not yet in the crate.
- *Acceptance:* NDI out visible in a receiver; one streaming protocol round-trips.

### Phase 10 — Recording *(greenfield)*
- **T10.1 [Port]** Per-output recorder over `rustjay-io/output` + ffmpeg: H.264,
  H.265, AV1, ProRes 422, HAP Q.
- *Acceptance:* record an output to a playable file in ≥2 codecs.

### Phase 11 — Persistence & presets
- **T11.1 [Port]** `.varda/` workspace: `scene.json`, `stage.json`, `midi.json`,
  `keymap.json`; Cmd+S + auto-save on clean exit; scene/stage separation.
- **T11.2 [Extend]** Deck/channel presets via `serialize_preset_state` /
  `deserialize_preset_state` / `on_preset_applied` + `presets/` dir.
- **T11.3 [Port]** `keymap` bindings layer.
- *Acceptance:* full round-trip restore at a different "venue" (swap stage, keep scene).

### Phase 12 — Transitions & sequencer
- **T12.1 [Reuse]** ISF shader transitions between channels; `AutoCrossfade` /
  `BeatSyncCrossfade`; deck auto-transitions (timer/clip-end).
- **T12.2 [Extend]** Multi-channel transition sequencer (beat-synced or timed:
  s/min/hr) for automated installs (`rustjay-mixer/sequencer.rs`).
- *Acceptance:* a beat-synced sequence and a long-timer sequence both run unattended.

### Phase 13 — Experimental: dome & edge-blend
- **T13.1 [Reuse]** Dome: fisheye→equirect (360°) + cubemap (`projection/dome.rs`),
  lens correction, chromatic aberration.
- **T13.2 [Reuse]** Edge blending: Auto (polygon overlap) + Manual per-edge
  (`edge_blend.rs`, `auto_blend.rs`); overlap zones.
- *Acceptance:* dome master renders; auto-detected overlap blends two surfaces.

### Phase 14 — Parity audit, perf, docs
- **T14.1** Walk the Phase-0 parity tracker to 100%; file gaps as follow-ups.
- **T14.2** Perf pass (heed [[project_perf_analysis_2026_05_23]]: no hard 120fps cap,
  StagingBelt, per-frame allocs; opacity-cull verified to skip GPU work).
- **T14.3** Update `guide/` (mdBook, [[project_guide]]) + the `rustjay` skill's
  Varda case study to match the shipped assembly.

---

## 5. GUI mapping (Varda panels → engine egui tabs)

Pattern (from `examples/delta-egui`): implement `AnyEguiTab`, `replaces()` a
built-in where appropriate, `downcast_mut::<Scene>()` the app state, and drive
parameters through `engine.get_param*` / `set_param_base` (so MIDI/OSC/LFO/HTTP
stay authoritative) plus `param_slider` / `param_slider_int` helpers.

| Varda panel | Tab | Notes |
|-------------|-----|-------|
| `mixer.rs` | **Mixer** | crossfader, per-channel opacity, master FX. |
| `deck_detail.rs` | **Deck** | source picker, opacity/blend/scaling, deck FX. |
| `effects.rs` + `library.rs` | **Effects / Library** | drag-add from registry, reorder. |
| `modulation.rs` | **Modulation** | LFO/audio/ADSR/step assignment + chaining graph. |
| `sequence.rs` | **Sequencer** | transition sequences. |
| `midi.rs` | **MIDI** | device select, learn/unlearn, mapping table. |
| `stage.rs` + `geometry.rs` | **Stage** | 2D surface editor, warp handles, import. |
| `outputs.rs` | **Outputs** | window/display/NDI/stream/record assignment. |
| `right_panel.rs` | **Inspector** | context panel for selected node. |
| `notifications*.rs` | overlay | toast layer (not a tab). |

Desktop entry:
```rust
rustjay_engine::run_with_egui_tabs(
    VardaPlugin::new(),
    vec![Box::new(MixerTab), Box::new(DeckTab), Box::new(EffectsTab),
         Box::new(ModulationTab), Box::new(SequencerTab), Box::new(MidiTab),
         Box::new(StageTab), Box::new(OutputsTab), Box::new(InspectorTab)],
)
```

---

## 6. Risks & open gaps

1. **Deck-per-channel multiplicity** — the engine's `Channel` holds one effect;
   Varda channels composite *many* decks. This is the single largest *port* (vs
   reuse). Validate the offscreen-per-deck → channel-composite cost early (Phase 1).
2. **Modulation parity** — engine has LFO + audio routing; ADSR, step-seq, and
   4-deep chaining must be ported and routed through core targets without forking
   the modulation engine.
3. **Codec/protocol coverage in `rustjay-io`** — HAP decode, SRT/HLS/DASH/RTMP, and
   recording are the least-certain reuse. Phase 2/9/10 each open with a coverage
   probe; budget a port fallback.
4. **Parameter-path scheme** — Varda's `deck/<uuid>/param/<name>` hierarchy is richer
   than the engine's flat param ids. Extend `ParameterDescriptor` addressing or
   namespace within the plugin; keep MIDI/OSC/HTTP/LFO all targeting the same paths.
5. **Two render-driver shapes** — imperative `render()` hook (current spine) vs
   declarative `render_graph()`. Stay imperative for the routing graph; reserve
   declarative passes for surfaces/projection where it fits.
6. **Egui tabs + projection entrypoint gap** — `run_with_egui_tabs` has no projection
   setup hook; `run_with_projection` uses imgui tabs, not egui. **Resolution:** a new
   combined entrypoint `run_with_projection_egui_tabs(plugin, tabs, setup)` was added
   to `rustjay-engine` (gated on `projection` + `egui`). It creates `App::new_with_egui`
   (sets `use_egui = true`) then runs the projection setup closure on
   `app.projection_subsystem` before the event loop. The frame loop already calls
   `sub.render()` after `engine.render()` when `feature = "projection"` is on,
   regardless of entrypoint. Varda's `main.rs` branches:
   - `projection` on → `run_with_projection_egui_tabs` with one identity projector
   - `projection` off → `run_with_egui_tabs` (Phase 6 behavior preserved)
   - no `egui`/`mixer` → `run()` fallback. Verified: `--no-default-features`,
     default, `--features projection`, `--all-features` all green.
7. **Feature-flag matrix** — NDI default-on broke Linux CI before
   ([[project_pr18_ci_parked]]). Gate NDI/streaming/recording off-by-default and
   keep a no-feature build green.
8. **Perf regressions** — multi-deck offscreen passes multiply submits; honor the
   per-file perf findings and verify opacity-culling actually elides GPU work.

---

## 7. Definition of done

- `cargo run -p varda` launches a desktop VJ app with the full routing graph, GUI,
  control, modulation, surfaces, multi-output, and persistence.
- The Phase-0 parity tracker is 100% against the Varda README capability list
  (experimental items flagged, not required).
- Headless + projection entrypoints render mapped outputs.
- No-default-feature build and Linux CI are green.
- `guide/` and the `rustjay` skill's Varda case study reflect the shipped app.
