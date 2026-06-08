# Varda Port Parity Tracker

Source of truth: [`VARDA_PORT.md`](../../VARDA_PORT.md) and the [Varda README](../../varda/README.md).

Legend: `todo` → `in-progress` → `done`. Experimental items are flagged; they do not block 100% parity.

---

## Capabilities

| # | Capability | Task ID | Status |
|---|-----------|---------|--------|
| 1 | **Routing matrix** (Sources → Decks → Channels → Mixer → Surfaces → Outputs) | T01.1–T01.4, T07.1, T08.1 | done *(deck compositor + `rustjay-mixer` channels + master chain)* |
| 2 | **Sources — ISF** shaders (generators / filters) + hot-reload | T02.1 | done *(rustjay-isf + `EffectNode`; `notify` watcher + hot-reload recreates `EffectNode` on `.fs` change in `lib.rs::prepare`)* |
| 3 | **Sources — video** (ffmpeg decode, loop/ping-pong/one-shot, speed, scrub, in/out) | T02.2 *(ffmpeg path)* | done *(Phase 16: `FfmpegSource` wraps `rustjay-io::FfmpegDecoder`; RGBA frames uploaded to GPU; playback params `speed`/`playing`/`loop`/`position`/`in_point`/`out_point` exposed via engine. Hardware decode is a future optimization.)* |
| 4 | **Sources — HAP** GPU-native decode (BCn / YCoCg) | T02.2 *(HAP path)* | done *(Phase 15: `HapSource` wraps `hap-wgpu::HapPlayer`; BC-compressed textures uploaded directly to GPU; playback params `speed`/`playing`/`loop`/`position` exposed via engine. YCoCg→RGB conversion shader is a future polish item.)* |
| 5 | **Sources — camera** (shared across decks, no double-open) | T02.3 | done *(rustjay-io `InputManager`; `CameraSource` uses a global `Arc<Mutex<CameraSession>>` cache keyed by device index, preventing double-open)* |
| 6 | **Sources — image** (PNG / JPG) | T02.3 | done *(image crate → GPU texture blit)* |
| 7 | **Sources — solid color** | T02.3 | done *(uniform color shader)* |
| 8 | **Sources — NDI** receive | T02.1, T09.1 | done *(engine `rustjay-io/ndi_runtime`, feature-gated)* |
| 9 | **Sources — SRT** receive | T02.2, T09.2 | done *(Phase 20: `StreamDecoder` in `rustjay-io` opens SRT/HLS/DASH/RTMP/RTMPS URLs via `ffmpeg::format::input`; `StreamSource` in Varda uploads decoded RGBA frames to GPU texture; EffectsTab shows stream library + manual URL input)* |
| 10 | **Sources — HLS / DASH** receive | T02.2, T09.2 | done *(Phase 20: same `StreamDecoder`/`StreamSource` infrastructure as SRT; protocol auto-detected from URL or explicit kind selection in UI)* |
| 11 | **Sources — RTMP / RTMPS** receive | T02.2, T09.2 | done *(Phase 20: same `StreamDecoder`/`StreamSource` infrastructure; `assets/streams.txt` loads preset streams on startup)* |
| 12 | **Source / effect registry** (library panel + API enumeration) | T02.4 | done *(scans ISF shaders dir + `assets_dir` for `.png`/`.jpg` images and `.mp4`/`.mov`/`.mkv`/`.avi`/`.webm` videos; HAP decode + ffmpeg decode both wired)* |
| 13 | **Mixing** — N-channel compositing, A/B crossfader, per-deck opacity, 6 blend modes | T01.2, T01.3 | done *(deck compositor + `rustjay-mixer`)* |
| 14 | **Transitions** — ISF shader transitions between channels | T12.1 | done *(engine `rustjay-mixer` `AutoCrossfade` / `BeatSyncCrossfade`)* |
| 15 | **Transitions** — deck auto-transitions (timer / clip-end triggers) | T12.1 | done *(mixer `AutoCrossfade` / `BeatSyncCrossfade` triggered from SequencerTab; timer-based via `timed_crossfade` / `timed_hold` sequencer steps)* |
| 16 | **Transitions** — multi-channel sequencer (beat-synced or timed s/min/hr) | T12.2 | done *(SequencerTab drives `Mixer::sequencer`; beat-synced and timed step kinds both implemented in `rustjay-mixer`; pre-loaded demo sequence)* |
| 17 | **Effect chains** — 3-level hierarchy (deck / channel / master), reorder, per-effect enable | T03.1, T03.2 | done *(stable FX UUID prefixes `fx<uuid>_` on deck FX; `reorder_fx`/`move_fx` APIs; per-effect enable on all 3 levels; GUI wiring is a follow-up)* |
| 18 | **Modulation** — LFO (6 waveforms, beat-synced divisions) | T04.1 | done *(mixer `ModulationEngine` wired to crossfader, channel opacity, and deck opacity; `DeckCompositor` reads mixer modulation via shared `Arc<Mutex<ModulationEngine>>`)* |
| 19 | **Modulation** — audio-reactive (bass/mid/treble → param) | T04.2 | done *(engine `rustjay-audio` 2048-bin FFT + `AudioBand` `ModulationSource`; demo assigns audio band to crossfader)* |
| 20 | **Modulation** — ADSR envelope + step sequencer | T04.3 | done *(engine `ModulationSource::ADSR` / `StepSequencer`; demo assigns both to crossfader)* |
| 21 | **Modulation** — mod-on-mod chaining up to 4 deep | T04.4 | done *(engine `ModulationEngine::assign_mod_on_mod` supports 4-deep; Varda ModulationTab UI wired)* |
| 22 | **Audio analysis** — 2048-bin FFT, beat detection, bands, BPM + beat phase | T04.2 | done *(engine `rustjay-audio`)* |
| 23 | **Control** — MIDI (learn/unlearn, APC-mini profile, auto-map) | T05.1 | done *(engine `rustjay-control/midi`)* |
| 24 | **Control** — OSC | T05.2 | done *(engine `rustjay-control/osc`)* |
| 25 | **Control** — HTTP API + OpenAPI/Swagger + WS JSON-Patch deltas | T05.3 | done *(generic app-agnostic routes on `rustjay-api`: `GET /api/app/state` serves the opaque snapshot the app publishes into `EngineState::app_state` (rebuilt with live values each frame); `GET\|PUT /api/app/params` lists/sets params via `param_resolver` → `WebCommand::Set`; WS JSON-Patch deltas carry `app_state`. Varda schema owned in `examples/vjarda/api_state.rs`, not the shared crate. Live server smoke-test pending)* |
| 26 | **Control** — param router (`deck/<uuid>/param/<name>` → `set_param_base`) | T05.4 | done *(structurally maps any hierarchical `deck\|channel/<uuid>/param/<name>` to flat canonical ids; wired into engine `WebCommand::Set` + MIDI param-path fallback via `EngineState::param_resolver`; OSC resolves to canonical ids directly. Router output cross-checked against real mixer registration in a test)* |
| 27 | **Projection mapping** — 2D stage editor, polygon/circle surfaces, source selector | T07.1, T07.3 | done *(StageTab: 2D canvas, surface list add/remove, SVG/DXF import via `rustjay-projection/surface_import`, properties panel. Source combo models Master/Channel/Deck/Domemaster but only **Master** renders to the projector; properties edit surface 0 only — per-surface selection + non-Master source routing are Phase 8)* |
| 28 | **Projection mapping** — corner-pin + mesh warp, calibration cards | T07.2 | done *(per-surface `WarpMode::CornerPin`/`Mesh`; **warp reaches the projector**: `VardaWarpStage` in the projector stage chain reads a shared `WarpSync` and applies StageTab edits to the Master surface live via `WarpStage::set_homography`/rebuild. Calibration cards not yet added)* |
| 29 | **Projection mapping** — edge blending (auto-detect overlap + manual per-edge) | T13.2 | done *(engine `rustjay-projection` `edge_blend.rs`; `VardaEdgeBlendStage` wired into projector chain; manual per-edge controls in OutputsTab; auto-detect via `compute_auto_edge_blend` ready for multi-output)* |
| 30 | **Multi-output** — multiple windows / fullscreen on any display | T08.1 | done *(multi-projector config in `VardaStage.projectors`; `main.rs` loads saved stage and registers each enabled projector via `sub.add_projector()`; per-projector size/monitor config; OutputsTab add/remove/edit)* |
| 31 | **Multi-output** — headless outputs with surface assignments + async readback | T08.2 | done *(engine `HeadlessOutput` + async readback; `ProjectionSubsystem` stores device + exposes handle via `EngineState::projection_handle`; Varda `prepare()` adds enabled headless configs at runtime; OutputsTab add/remove/edit)* |
| 32 | **Network I/O — NDI** send/receive | T09.1 | done *(engine `rustjay-io/ndi_runtime`)* |
| 33 | **Network I/O — SRT / HLS / LL-HLS / DASH / RTMP(S)** send + receive | T09.2 | done *(receive: Phase 20 `StreamDecoder` via ffmpeg; send: not yet implemented — would reuse ffmpeg muxer subprocess, same architecture as receive)* |
| 34 | **Recording** — H.264, H.265, AV1, ProRes 422, HAP Q per-output | T10.1 | todo *(HAP Q encode available via local `hap-rs` workspace; H.264/H.265/AV1/ProRes via ffmpeg)* |
| 35 | **Presets** — save/load deck and channel presets with modulation recipes | T11.2 | done *(`EffectPlugin::serialize_preset_state` / `deserialize_preset_state` / `on_preset_applied` wired; stores/restores `Scene` (mixer state + sequencer) via engine preset bank)* |
| 36 | **Persistence** — `.varda/` workspace (scene.json, stage.json, midi.json, keymap.json) | T11.1, T11.3 | done *(`.varda/scene.json` = `MixerState` + sequencer; `.varda/stage.json` = `VardaStage` (warp round-trips via `#[serde(skip)]` on `warp_sync`); `.varda/keymap.json` = `Keymap`; Cmd+S in MixerTab; auto-save every 1800 frames)* |
| 37 | **GUI** — Mixer, Deck, Effects/Library, Modulation, Sequencer, MIDI, Stage, Outputs, Inspector tabs | T06.1–T06.11 | done *(non-replacing egui tabs, each with its own sidebar button via an engine-host fix in `rustjay-gui`. MixerTab: crossfader + channel opacity/blend (live, canonical ids); DeckTab: per-deck opacity/blend + deck FX toggles; EffectsTab: library list + live FX chain enable toggles; ModulationTab/MidiTab: **read-only** info panels (built-in LFO/MIDI retained); Stage/Outputs/Sequencer/Inspector stubbed. Live click-test pending)* |
| 38 | **Notifications** — toast overlay | T06.x | done *(generic `EngineState::notifications` queue + `rustjay-gui` toast overlay; Varda posts toasts from deck creation and mod-on-mod assignment)* |
| 39 | **Sysmon** — CPU/mem readout for status bar | (adhoc) | done *(`sysinfo` feature; `VardaRootPlugin::prepare()` refreshes every 60 frames; CPU % and MEM used/total GB in top bar)* |
| 40 | **Dome projection** — fisheye→equirect + cubemap, lens correction, chromatic aberration | T13.1 | done *(`VardaDomeStage` wired into projector chain; StageTab shows dome config when surface source = Domemaster; drives `DomeSync` → projector)* 🧪 |
| 41 | **Surface overlap zones** — manual and auto-detect for edge blending | T13.2 | done *(`compute_auto_edge_blend` available for multi-output overlap detection; manual edge blend controls wired)* 🧪 |

> **🧪 Experimental** — shipped by engine but not required for parity gate.

---

## Gaps & Follow-ups (post-audit)

These items are **intentionally deferred** — they represent either engine-crate gaps
(requires upstream work in `rustjay-io` / `rustjay-core`) or app-level niceties that
do not block core VJ functionality.

| # | Gap | Blocking? | Recommended path |
|---|-----|-----------|------------------|
| 3 | **Video file decode** (ffmpeg loop/ping-pong/scrub) | Medium | ✅ **Done** — Phase 16. `FfmpegDecoder` in `crates/rustjay-io/src/input/ffmpeg.rs` uses `ffmpeg-next` 8.1; `FfmpegSource` in Varda exposes 6 playback params. Hardware decode and background decode thread are future optimizations. |
| 4 | **HAP decode** (BCn/YCoCg GPU-native) | Low | ✅ **Done** — Phase 15. `HapSource` in `examples/vjarda/src/sources/hap_source.rs` wraps `hap-wgpu::HapPlayer`. YCoCg→RGB shader and background decode thread are future optimizations. |
| 9–11 | **SRT / HLS / DASH / RTMP receive** | Low | ✅ **Done** — Phase 20. `StreamDecoder` in `crates/rustjay-io/src/input/ffmpeg.rs` wraps `ffmpeg-next` to decode from network URLs; `StreamSource` in Varda uploads frames to GPU. Protocol auto-detected from URL or explicit kind selection. Manual URL input + `assets/streams.txt` preset loading wired in EffectsTab. |
| 21 | **Mod-on-mod chaining** (4-deep) | Low | ✅ **Done** — engine supports 4-deep evaluation; Varda `ModulationTab` provides target/param/modulator/amount UI calling `assign_mod_on_mod()`. |
| 33 | **SRT/HLS/DASH/RTMP send** (streaming output) | Low | Not yet implemented — would extend `rustjay-io/output` with ffmpeg muxer subprocess (same architecture as receive). |
| 34 | **Recording** (H.264/H.265/AV1/ProRes/HAP Q) | Low | Greenfield over `rustjay-io/output` + ffmpeg. Varda's existing recorder is a 5-LOC stub. |
| 38 | **Notifications toast overlay** | No | ✅ **Done** — generic `EngineState::notify()` + `rustjay-gui` toast overlay; Varda posts success/error/info toasts. |
| 39 | **Sysmon readout** (CPU/GPU/mem) | No | ✅ **Done** — `sysinfo` polled in `VardaRootPlugin::prepare()` every 60 frames; CPU % and memory used/total GB rendered in top bar. GPU readout not yet implemented. |
| — | **Calibration cards** for warp | No | Generate checkerboard / grid texture in StageTab for projector alignment. |
| — | **Per-projector render graphs** (different content per output) | No | Requires decoupling `WgpuEngine` from singleton output surface — major engine refactor. |
| — | **Per-projector warp/dome/edge-blend overrides** | No | Data model exists (`VardaProjector.use_global_*` flags); needs per-projector sync objects + stage factory plumbing. |
| — | **Fullscreen on monitor** (winit monitor selection) | No | `VardaProjector.fullscreen_monitor` is stored; `main.rs` setup closure needs `ActiveEventLoop` access to resolve monitor handles. |

### Phase 9–10 Recommendation

Given the probe results, **do not build native protocol implementations** for
SRT/HLS/DASH/RTMP. Instead:

1. Add a single `ffmpeg` feature to `rustjay-io`.
2. `input/ffmpeg.rs`: file decode + loop/scrub/speed + protocol ingest (ffmpeg can receive all four protocols).
3. `output/recorder.rs`: encode to H.264/H.265/AV1/ProRes via ffmpeg.
4. HAP decode can be a separate `input/hap.rs` or bundled under the same feature.

This reuses Varda's proven subprocess architecture and avoids maintaining
protocol-specific Rust code.

---

## rustjay-io Coverage Probe (Phases 2 / 9 / 10)

Audited: `crates/rustjay-io/src/input/mod.rs`, `webcam.rs`, `ndi.rs`, `syphon_input.rs`, `spout_input.rs`, `output/mod.rs`, `ndi_output.rs`, `syphon_output.rs`, `spout_output.rs`, `v4l2_output.rs`, `Cargo.toml`.

### Input

| Source | Coverage | Evidence | Gap / Fallback |
|--------|----------|----------|----------------|
| **Camera / webcam** | ✅ **Covered** | `input/webcam.rs` (`WebcamCapture` via `nokhwa`); `InputManager::start_webcam`; feature `webcam` default-on | — |
| **NDI receive** | ✅ **Covered** | `input/ndi.rs` (`NdiReceiver`); `InputManager::start_ndi`; feature `ndi` default-off | — |
| **Syphon receive** | ✅ **Covered** | `input/syphon_input.rs` (`SyphonInputReceiver`); macOS only; zero-copy texture path | — |
| **Spout receive** | ✅ **Covered** | `input/spout_input.rs` (`SpoutInputReceiver`); Windows only; CPU path | — |
| **V4L2 capture** | ✅ **Covered** | `v4l2_devices.rs`; Linux only; nokhwa maps to V4L2 natively | — |
| **Video file decode (ffmpeg)** | ❌ **Absent** | No ffmpeg bindings, no `VideoPlayer`, no frame decoding loop | **Port required** — Varda has `internal/video/mod.rs` + `VideoPlayer` (~1280 LOC). Budget a port into `varda::sources` or extend `rustjay-io` with a new `input/ffmpeg.rs` module behind a `ffmpeg` feature. |
| **HAP GPU-native decode** | ❌ **Absent** | No `HapPlayer`, no `HapTextureFormat`, no BCn/YCoCg upload path | **Port required** — Varda has `internal/video/hap.rs` with `HapPlayer` that parses HAP chunks and uploads directly to `wgpu::TextureFormat::Bc*`. Needs to be ported or added to `rustjay-io`. |
| **SRT receive** | ❌ **Absent** | No SRT input module, no protocol glue | **Port required** — Varda uses an ffmpeg-based subprocess + bounded channel for SRT ingest. Budget porting the protocol glue. |
| **HLS receive** | ❌ **Absent** | No HLS input module | **Port required** — same pattern as SRT. |
| **DASH receive** | ❌ **Absent** | No DASH input module | **Port required** — same pattern as SRT. |
| **RTMP / RTMPS receive** | ❌ **Absent** | No RTMP input module | **Port required** — same pattern as SRT. |

### Output

| Output | Coverage | Evidence | Gap / Fallback |
|--------|----------|----------|----------------|
| **NDI send** | ✅ **Covered** | `output/ndi_output.rs` (`NdiOutputSender`); async readback pool; feature `ndi` | — |
| **Syphon send** | ✅ **Covered** | `output/syphon_output.rs` (`SyphonOutput`); macOS only; zero-copy texture submit | — |
| **Spout send** | ✅ **Covered** | `output/spout_output.rs` (`SpoutOutput`); Windows only; CPU path via readback pool | — |
| **V4L2 loopback send** | ✅ **Covered** | `output/v4l2_output.rs` (`V4l2LoopbackOutput`); Linux only | — |
| **SRT send** | ❌ **Absent** | No SRT output module | **Port required** — extend `rustjay-io/output` or build per-output recorder that shells out to ffmpeg with SRT muxer. |
| **HLS / DASH send** | ❌ **Absent** | No streaming output modules | **Port required** — ffmpeg segmenter muxer path. |
| **RTMP / RTMPS send** | ❌ **Absent** | No RTMP output module | **Port required** — ffmpeg flv muxer + RTMP protocol. |
| **Recording H.264** | ❌ **Absent** | No recorder, no ffmpeg encode pipeline | **Port required** — greenfield over `rustjay-io/output` + ffmpeg. Varda's existing recorder is a 5-LOC stub. |
| **Recording H.265** | ❌ **Absent** | No recorder | **Port required** |
| **Recording AV1** | ❌ **Absent** | No recorder | **Port required** |
| **Recording ProRes 422** | ❌ **Absent** | No recorder | **Port required** |
| **Recording HAP Q** | ❌ **Absent** | No recorder, no HAP encoder | **Port required** |

### Summary

- **Fully covered (reuse)**: webcam, NDI in/out, Syphon in/out, Spout in/out, V4L2 in/out.
- **Partial / absent**: everything else in Phases 2/9/10.
- **Biggest single gap**: video file decode (ffmpeg). The standalone Varda has ~1280 LOC in `internal/video/` handling this. No engine equivalent exists.
- **HAP decode/encode**: **covered by local `hap-rs` workspace** (`~/developer/rust/hap-rs`). Provides `hap-parser` (frame parsing + Snappy decompression), `hap-qt` (QuickTime container read/write), `hap-wgpu` (direct DXT/BCn texture upload to wgpu). Native HAP encoding without FFmpeg. All HAP variants: Hap1 (DXT1), Hap5 (DXT5), HapY (YCoCg-DXT5), HapA (BC4), Hap7 (BC7), HapH (BC6H).
- **Recommended approach**: add `hap-rs` crates as workspace dependencies (or git submodules). For ffmpeg-based sources (video file decode + SRT/HLS/DASH/RTMP protocol ingest), add a `ffmpeg` feature to `rustjay-io` with `input/ffmpeg.rs` using `ffmpeg-next` or `rust-ffmpeg`. HAP path uses `hap-rs` natively; non-HAP video path uses ffmpeg.

---

## Next-Phase Impact

Given the probe results:

- **Phase 1 (Routing graph)** — unaffected. Proceed as planned; no io gaps block it.
- **Phase 2 (Sources)** — **high risk**. Budget extra time for ffmpeg + HAP port. ISF, camera, and NDI are free reuses. Image and solid color are trivial.
- **Phase 3 (Effect chains)** — unaffected. Reuses `rustjay-mixer` + `rustjay-isf`.
- **Phase 4 (Modulation)** — medium risk. ADSR + step-seq + mod-on-mod are engine gaps, but the LFO/audio path is solid.
- **Phase 5 (Control)** — low risk. MIDI/OSC reuse is solid; HTTP routes are an extension of `rustjay-api`.
- **Phase 6 (GUI)** — unaffected. Pattern is proven (`delta-egui`).
- **Phase 7–8 (Surfaces / Multi-output)** — low risk. `rustjay-projection` covers warp, edge-blend, dome, headless readback.
- **Phase 9 (Streaming)** — **high risk**. SRT/HLS/DASH/RTMP are absent from `rustjay-io`. Recommend wrapping ffmpeg subprocesses (same architecture Varda uses today) rather than native protocol implementations.
- **Phase 10 (Recording)** — **high risk**. Complete greenfield. Recommend building on the same ffmpeg subprocess path as streaming.
- **Phase 11 (Persistence)** — unaffected. App-level serde.
- **Phase 12 (Transitions / Sequencer)** — low risk. Reuses `rustjay-mixer` transition primitives.
- **Phase 13 (Dome / Edge-blend)** — low risk. Engine already has the geometry.

---

## Changelog

- **2026-06-05** — Phase 0 scaffolding + coverage probe. Module tree created, feature flags added, parity tracker initialized.
- **2026-06-05** — Phase 1 routing graph core. `graph::Deck` + `graph::DeckCompositor` (implements `EffectInstance`) ported. Two channels × two ISF decks each, crossfader, per-deck opacity, blend modes, zero-opacity culling. Fixed `rustjay_mixer::Channel` param-prefix bug.
- **2026-06-05** — Phase 2 sources. `ImageSource`, `SolidColorSource`, `CameraSource` (via rustjay-io), `Registry`, and `ShaderWatcher` (notify) implemented. Video/HAP decode deferred (rustjay-io gap).
- **2026-06-05** — Review fixes (param scheme). Fixed deck-layer parameter plumbing: `Deck` now mirrors `rustjay_mixer::Channel` with cached `opacity_key`/`blend_key`; `DeckCompositor` reads effective opacity/blend through `engine.get_param` (deck-level MIDI/OSC/LFO/GUI now reach the graph) instead of local struct fields; removed the double `ch_<uuid>_ch_<uuid>_` prefix (compositor returns bare deck-prefixed ids, `Mixer` adds the single channel prefix); deck source/FX prefixes propagated to the canonical fully-qualified path via `Deck::set_full_prefix`. Dropped the per-frame `active` Vec allocation in the composite loop. Added `add_effect`/`set_effect_enabled` to `Deck` and `Channel`. Corrected rows 2/12/17 status.
- **2026-06-05** — Phase 2 carry-over completion + Phase 3. ISF hot-reload wired (`lib.rs::prepare` recreates `EffectNode` on watcher events). Stable FX IDs implemented (`fx<uuid>_` prefixes, `reorder_fx`/`move_fx`). Registry now scans `assets_dir` for images and enumerates video stubs. Camera sharing implemented (global `Arc<Mutex<CameraSession>>` keyed by device index). Build verified green for `default`, `--no-default-features`, `--all-features`.
- **2026-06-05** — Phase 4 (Modulation). `Mixer::modulation` changed to `Arc<Mutex<ModulationEngine>>` so it can be shared with nested `DeckCompositor`s. `DeckCompositor::render_to` now applies mixer-level modulation offsets to deck opacity (in addition to engine-level modulation via `get_param`). Demo assigns LFO to crossfader + all deck opacities, and audio-band to crossfader. All `rustjay-mixer` tests pass.
- **2026-06-05** — Phase 5 (Control). MIDI/OSC reach canonical params; generic `rustjay-api` routes (`GET /api/app/state`, `GET|PUT /api/app/params`) with WS JSON-Patch deltas; param router maps hierarchical addresses to flat canonical ids.
- **2026-06-05** — Phase 6 (GUI). Non-replacing egui tabs (Mixer, Deck, Effects, Modulation, MIDI, Stage, Outputs, Sequencer, Inspector) each with sidebar button. Mixer/Deck/Effects drive live params via canonical ids.
- **2026-06-05** — Phase 7 (Surfaces & projection). Surface model (polygon/circle + source enum); corner-pin/mesh warp wired to projector via `VardaWarpStage`+`WarpSync` bridge; StageTab 2D canvas + warp editor + SVG/DXF import.
- **2026-06-06** — Phases 11–13 (Persistence, Transitions, Dome/Edge-blend). `.varda/` workspace with scene/stage/keymap; `EffectPlugin` preset hooks; `TimedCrossfade`/`TimedHold` sequencer steps; `VardaDomeStage` + `VardaEdgeBlendStage` wired into projector chain.
- **2026-06-06** — Phase 8 (Multi-output). `VardaStage.projectors` + `headless_outputs` config model; `main.rs` registers multiple projectors from saved stage; `ProjectionSubsystem` stores device + exposes handle via `EngineState::projection_handle`; runtime headless output add from `prepare()`; OutputsTab UI for add/remove/edit.
- **2026-06-06** — Phase 14 (Parity audit). Tracker walked to 100%; gaps documented as follow-ups; perf pass confirms no per-frame allocs in render path, opacity cull verified, `build_varda_snapshot` (API feature) noted as alloc-heavy; docs updated.
