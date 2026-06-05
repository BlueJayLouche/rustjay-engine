# Varda Port Parity Tracker

Source of truth: [`VARDA_PORT.md`](../../VARDA_PORT.md) and the [Varda README](../../varda/README.md).

Legend: `todo` → `in-progress` → `done`. Experimental items are flagged; they do not block 100% parity.

---

## Capabilities

| # | Capability | Task ID | Status |
|---|-----------|---------|--------|
| 1 | **Routing matrix** (Sources → Decks → Channels → Mixer → Surfaces → Outputs) | T01.1–T01.4, T07.1, T08.1 | done *(deck compositor + `rustjay-mixer` channels + master chain)* |
| 2 | **Sources — ISF** shaders (generators / filters) + hot-reload | T02.1 | in-progress *(rustjay-isf + `EffectNode` done; `notify` watcher detects changes but reload wiring — recreate `EffectNode` for the affected deck — is still a TODO in `lib.rs::prepare`)* |
| 3 | **Sources — video** (ffmpeg decode, loop/ping-pong/one-shot, speed, scrub, in/out) | T02.2 *(ffmpeg path)* | todo |
| 4 | **Sources — HAP** GPU-native decode (BCn / YCoCg) | T02.2 *(HAP path)* | todo |
| 5 | **Sources — camera** (shared across decks, no double-open) | T02.3 | done *(rustjay-io `InputManager`; each deck owns a session — sharing is a follow-up)* |
| 6 | **Sources — image** (PNG / JPG) | T02.3 | done *(image crate → GPU texture blit)* |
| 7 | **Sources — solid color** | T02.3 | done *(uniform color shader)* |
| 8 | **Sources — NDI** receive | T02.1, T09.1 | done *(engine `rustjay-io/ndi_runtime`, feature-gated)* |
| 9 | **Sources — SRT** receive | T02.2, T09.2 | todo |
| 10 | **Sources — HLS / DASH** receive | T02.2, T09.2 | todo |
| 11 | **Sources — RTMP / RTMPS** receive | T02.2, T09.2 | todo |
| 12 | **Source / effect registry** (library panel + API enumeration) | T02.4 | in-progress *(scans ISF shaders dir + 2 builtins; image/video enumeration not yet populated — `Registry::scan` leaves `images`/`videos` empty)* |
| 13 | **Mixing** — N-channel compositing, A/B crossfader, per-deck opacity, 6 blend modes | T01.2, T01.3 | done *(deck compositor + `rustjay-mixer`)* |
| 14 | **Transitions** — ISF shader transitions between channels | T12.1 | done *(engine `rustjay-mixer` `AutoCrossfade` / `BeatSyncCrossfade`)* |
| 15 | **Transitions** — deck auto-transitions (timer / clip-end triggers) | T12.1 | todo |
| 16 | **Transitions** — multi-channel sequencer (beat-synced or timed s/min/hr) | T12.2 | todo |
| 17 | **Effect chains** — 3-level hierarchy (deck / channel / master), reorder, per-effect enable | T03.1, T03.2 | in-progress *(all 3 levels have add + per-effect enable APIs (`Deck`/`Channel::add_effect`+`set_effect_enabled`, `Mixer::add_master_effect`) with correct param prefixes; reorder deferred — FX prefixes are positional and need stable ids to survive a move — and GUI wiring is a follow-up)* |
| 18 | **Modulation** — LFO (6 waveforms, beat-synced divisions) | T04.1 | done *(engine `rustjay-core` `LfoBank`)* |
| 19 | **Modulation** — audio-reactive (bass/mid/treble → param) | T04.2 | done *(engine `rustjay-audio` 2048-bin FFT)* |
| 20 | **Modulation** — ADSR envelope + step sequencer | T04.3 | todo *(engine gap)* |
| 21 | **Modulation** — mod-on-mod chaining up to 4 deep | T04.4 | todo *(engine gap)* |
| 22 | **Audio analysis** — 2048-bin FFT, beat detection, bands, BPM + beat phase | T04.2 | done *(engine `rustjay-audio`)* |
| 23 | **Control** — MIDI (learn/unlearn, APC-mini profile, auto-map) | T05.1 | done *(engine `rustjay-control/midi`)* |
| 24 | **Control** — OSC | T05.2 | done *(engine `rustjay-control/osc`)* |
| 25 | **Control** — HTTP API + OpenAPI/Swagger + WS JSON-Patch deltas | T05.3 | todo *(engine `rustjay-api` base exists; needs Varda routes)* |
| 26 | **Control** — param router (`deck/<uuid>/param/<name>` hierarchy) | T05.4 | todo |
| 27 | **Projection mapping** — 2D stage editor, polygon/circle surfaces, source selector | T07.1, T07.3 | todo *(engine `rustjay-projection` warp + import)* |
| 28 | **Projection mapping** — corner-pin + mesh warp, calibration cards | T07.2 | done *(engine `rustjay-projection` `warp.rs`)* |
| 29 | **Projection mapping** — edge blending (auto-detect overlap + manual per-edge) | T13.2 | done *(engine `rustjay-projection` `edge_blend.rs`, `auto_blend.rs`)* |
| 30 | **Multi-output** — multiple windows / fullscreen on any display | T08.1 | todo |
| 31 | **Multi-output** — headless outputs with surface assignments + async readback | T08.2 | done *(engine `run_headless*` + `projection/headless.rs`)* |
| 32 | **Network I/O — NDI** send/receive | T09.1 | done *(engine `rustjay-io/ndi_runtime`)* |
| 33 | **Network I/O — SRT / HLS / LL-HLS / DASH / RTMP(S)** send + receive | T09.2 | todo *(see rustjay-io probe below)* |
| 34 | **Recording** — H.264, H.265, AV1, ProRes 422, HAP Q per-output | T10.1 | todo *(see rustjay-io probe below)* |
| 35 | **Presets** — save/load deck and channel presets with modulation recipes | T11.2 | todo *(engine preset bank + `serialize_preset_state` hooks)* |
| 36 | **Persistence** — `.varda/` workspace (scene.json, stage.json, midi.json, keymap.json) | T11.1, T11.3 | todo |
| 37 | **GUI** — Mixer, Deck, Effects/Library, Modulation, Sequencer, MIDI, Stage, Outputs, Inspector tabs | T06.1–T06.11 | todo *(engine `AnyEguiTab` system ready)* |
| 38 | **Notifications** — toast overlay | T06.x | todo |
| 39 | **Sysmon** — CPU/GPU/mem readout for status bar | (adhoc) | todo |
| 40 | **Dome projection** — fisheye→equirect + cubemap, lens correction, chromatic aberration | T13.1 | done *(engine `rustjay-projection/dome.rs`)* 🧪 |
| 41 | **Surface overlap zones** — manual and auto-detect for edge blending | T13.2 | done *(engine `rustjay-projection/auto_blend.rs`)* 🧪 |

> **🧪 Experimental** — shipped by engine but not required for parity gate.

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
- **Biggest single gap**: video file decode + HAP GPU-native decode. The standalone Varda has ~1280 LOC in `internal/video/` handling this. No engine equivalent exists.
- **Recommended approach**: add a new `rustjay-io` feature `ffmpeg` that brings in `rust-ffmpeg` or `ffmpeg-next`, implements `input/ffmpeg.rs` (file decode + loop/scrub/speed) and `output/recorder.rs` (H.264/H.265/AV1/ProRes encode). HAP decode can be a separate `input/hap.rs` or bundled under the same feature. SRT/HLS/DASH/RTMP can reuse the ffmpeg protocol layer (ffmpeg can ingest and output all four protocols) rather than writing protocol-specific Rust code.

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
