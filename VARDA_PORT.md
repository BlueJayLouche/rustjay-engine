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

| Phase | State | Notes |
|-------|-------|-------|
| **0 — Scaffolding & parity harness** | ✅ done (T00.4 golden-image harness deferred) | Module tree, feature flags, parity tracker, rustjay-io coverage probe all in place. Golden-image diff harness (T00.4) not yet wired. |
| **1 — Routing graph core** | ✅ done | Deck → DeckCompositor → `rustjay_mixer::Channel` → Mixer spine runs. Zero-opacity culling **verified to skip the GPU pass** (`compositor.rs`, not multiply-by-zero). Deck opacity/blend now read through `engine.get_param` (post-review fix). |
| **2 — Sources** | 🟡 partial | ISF, camera, image, solid color, registry, `notify` watcher implemented. **Carry-overs:** ISF hot-reload *reload wiring* is a TODO (watcher only detects); registry does not enumerate images/videos; camera not yet shared across decks (double-open); video/HAP/SRT/HLS/DASH/RTMP absent (rustjay-io gap — port required, see PARITY probe). |
| **3 — Effect chains** | 🟡 partial | `add_effect` + `set_effect_enabled` on `Deck` and `Channel`; `Mixer::add_master_effect` for master; per-effect enable honored in all three render paths; params reachable via canonical prefixes. **Carry-overs:** FX **reorder** deferred (positional prefixes need stable FX ids); deck/channel FX chains have no GUI and are not exercised in the demo assembly. |
| **4–14** | ⬜ not started | Modulation, control, GUI, surfaces, multi-output, streaming, recording, persistence, transitions, dome/edge-blend, parity audit. |

### Carry-over backlog (must be cleared before the relevant phase is "done")

- **T02.1b** Wire ISF hot-reload: on a watcher event, recreate the affected deck's `EffectNode` (`lib.rs::prepare` has the TODO).
- **T02.4b** Populate `Registry::scan` `images`/`videos` (image/video file enumeration), so the Library lists more than ISF + 2 builtins.
- **T02.3b** Share one camera `InputManager` session across decks (no double-open).
- **T03.1b** FX reorder with stable ids: replace positional `fx<index>_` prefixes with stable per-effect ids so a move doesn't re-map param values; then add reorder APIs on `Deck`/`Channel`.
- **T00.4** Golden-image harness for headless render diffs (Phase-0 acceptance gate, deferred).

### Conventions established (do not regress)

- **Parameter scheme:** every level's `parameters()` returns ids prefixed with **only its own component**; the enclosing level adds its prefix (`Mixer` adds `ch_<uuid>_`, `DeckCompositor` decks add `deck_<uuid>_`). At render, read modulated values through `engine.get_param(&cached_key)` — **never** from local struct fields. `Deck` mirrors `rustjay_mixer::Channel` (cached `opacity_key`/`blend_key`, `set_full_prefix` propagation). This is the fix for the "modulation does nothing" class of bug; keep new nodes consistent.
- **Perf:** no per-frame allocations in `render`/`prepare`/`build_uniforms` (reuse scratch buffers, cache key strings); zero-opacity layers must skip the pass.
- **Features:** heavy features (`ndi`/`api`/`projection`/streaming/recording/syphon) stay off by default; the `--no-default-features` build stays green and warning-clean.

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

### Phase 5 — Control (co-equal consumers)
- **T05.1 [Reuse]** MIDI via `rustjay-control/midi` (learn, unlearn, APC-mini,
  auto-map). Bridge to Varda param paths.
- **T05.2 [Reuse]** OSC via `rustjay-control/osc`.
- **T05.3 [Extend]** HTTP/WS via `rustjay-api`: add `decks/channels/scene/surfaces/
  stage/sequences/library/effects` routes + WS JSON-Patch deltas; keep single
  listener (merged into `rustjay-control` web server).
- **T05.4 [Extend]** `param_router` bridging incoming control → `set_param_base`.
- *Acceptance:* same parameter driven identically from MIDI, OSC, and HTTP; Swagger
  UI lists the new routes; WS pushes deltas.

### Phase 6 — GUI (egui tabs) — see §5
- **T06.1–T06.11** one task per panel (Mixer, Decks/DeckDetail, Effects/Library,
  Modulation, Sequence, MIDI, Stage, Outputs, Geometry, RightPanel, Notifications).
- *Acceptance:* full live control of the graph from the desktop UI; built-in tabs
  Varda supersedes are hidden via `hidden_tabs()`/`replaces()`.

### Phase 7 — Surfaces & projection mapping
- **T07.1 [Extend]** Surface model (polygon/circle) + source selector (Master /
  Channel / multi-Channel sub-mix / Deck / Domemaster) over `rustjay-projection`.
- **T07.2 [Reuse]** Corner-pin + mesh warp (`warp.rs`), calibration cards.
- **T07.3 [Extend]** Stage editor tab (2D), `surface_import.rs` (SVG/DXF contours).
- *Acceptance:* draw surfaces, assign sources, corner-pin warp, import a contour.

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
6. **Feature-flag matrix** — NDI default-on broke Linux CI before
   ([[project_pr18_ci_parked]]). Gate NDI/streaming/recording off-by-default and
   keep a no-feature build green.
7. **Perf regressions** — multi-deck offscreen passes multiply submits; honor the
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
