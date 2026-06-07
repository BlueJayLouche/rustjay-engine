# Unified Modulation Roadmap — Single Source of Truth for LFO, ADSR, Audio & Sequencer

**Date:** 2026-06-06
**Scope:** `rustjay-engine` workspace + `examples/*`
**Companion docs:** `PHASE_B_ROADMAP.md` (B2.4 gap), `VARDA_PORT.md` (T04), `WAAAVES_PORT.md`
**Status:** Phase 1 & 2 complete; Phase 3 (modulation-aware `get_param`) in progress

---

## 1. Context

Today the engine has **two modulation systems** that do not interoperate:

| System | Location | Strengths | Weaknesses |
|---|---|---|---|
| **Legacy `LfoBank`** | `rustjay-core/src/lfo.rs` | Tempo sync, beat divisions, quantum-boundary phase snap, built-in GUI tab | Fixed 8 slots, 1:1 targeting, no ADSR/sequencer |
| **New `ModulationEngine`** | `rustjay-core/src/modulation.rs` | UUID-stable sources, multi-target assignments, ADSR, step-sequencer, audio band, mod-on-mod chaining | LFO is Hz-only; no tempo sync; not wired to engine HSB/custom params |

`rustjay-mixer` owns its own `Arc<Mutex<ModulationEngine>>`. The engine's built-in `update_lfo()` mutates `custom_params` and `hsb_params` directly. Varda (`examples/varda`) also owns a mixer-level `ModulationEngine` and manually adds offsets on top of `engine.get_param()`. Nothing is shared, nothing is authoritative, and the B2.4 gap is documented but unclosed.

This roadmap collapses both paths into **one engine-global `ModulationEngine`** that every subsystem reads through `engine.get_param()`.

---

## 2. Goals & Non-Goals

### Goals
- [x] `ModulationSource::LFO` gains `tempo_sync`, `division`, and `phase_offset_degrees` (parity with old `LfoBank`).
- [x] `ModulationEngine::update()` accepts BPM + beat phase so tempo-sync LFOs snap and re-sync.
- [x] `EngineState` owns **one** `Arc<Mutex<ModulationEngine>>` — the single source of truth.
- [ ] `EngineState::get_param(id)` transparently returns `base + modulation_offset` for every param. *(Phase 3)*
- [ ] `rustjay-mixer` drops its owned modulation engine; reads modulated values via `engine.get_param()`. *(Phase 4)*
- [ ] Built-in LFO tab becomes a **Modulation** tab that edits the shared `ModulationEngine`. *(Phase 5)*
- [x] Every example compiles (`waaaves` via deprecated shim). Runtime behaviour verification pending Phase 3.
- [x] Old `LfoBank` presets migrate automatically via `LfoBank::to_modulation_engine()` fallback in `Preset::apply_to_state`.

### Non-Goals
- **Audio routing matrix** (`AudioRoutingState`) is **not** absorbed yet — it stays separate but may be deprecated later.
- **No new crates** — everything lands in `rustjay-core`, `rustjay-engine`, `rustjay-mixer`, and `rustjay-gui`.
- **No GUI redesign** — the new Modulation tab reuses existing egui/imgui patterns; fancy drag-and-drop graphs are future work.
- **No WebSocket/API protocol changes** in this roadmap — the generic `/api/app/params` route already works.

---

## 3. Data Model — Before & After

### Before (today)

```
EngineState
├── lfo: LfoState
│   └── bank: LfoBank
│       └── lfos: Vec<Lfo>  (8 fixed slots)
│           └── target: LfoTarget  (HueShift | Saturation | Brightness | Custom | None)
├── hsb_params / hsb_param_bases
├── custom_params / custom_param_bases
├── audio: AudioState
└── audio_routing: AudioRoutingState

rustjay-mixer::Mixer
└── modulation: Arc<Mutex<ModulationEngine>>  (owned, separate)
    └── sources: Vec<ModulationSourceEntry>
        └── LFO { frequency, phase, amplitude, bipolar }  (no tempo_sync)
```

### After (target)

```
EngineState
├── modulation: Arc<Mutex<ModulationEngine>>  (single source of truth)
│   └── sources: Vec<ModulationSourceEntry>
│       └── LFO { frequency, phase, amplitude, bipolar,
│                   tempo_sync, division, phase_offset_degrees }
│       └── AudioBand { ... }
│       └── ADSR { ... }
│       └── StepSequencer { ... }
│   └── assignments: HashMap<param_id, Vec<ParamModulation>>
├── hsb_param_bases
├── hsb_params   (modulated each frame by reading "hue_shift"/"saturation"/"brightness" from engine)
├── custom_param_bases
├── custom_params   (modulated on-demand via get_param)
└── audio: AudioState   (fed into ModulationEngine as AudioValues)

rustjay-mixer::Mixer
└── (no owned modulation — reads via engine.get_param)
```

---

## 4. Phased Task Breakdown

### Phase 1 — Tempo-sync LFO in `ModulationEngine` *(Low risk, 1–2 days)*

Extend the LFO source so it can replace the old `LfoBank` feature-for-feature.

| Task | File | Acceptance |
|---|---|---|
| **M1.1** Add `tempo_sync: bool`, `division: usize`, `phase_offset_degrees: f32` to `ModulationSource::LFO`. | `rustjay-core/src/modulation.rs` | Compiles; existing unit tests pass. |
| **M1.2** Update `LFO::calculate()` to compute effective frequency from `division` + BPM when `tempo_sync=true`. Reuse `beat_division_to_hz()`. | `rustjay-core/src/modulation.rs` | Unit test: at 120 BPM, division=2 (1/4 note) produces 2 Hz. |
| **M1.3** Implement quantum-boundary phase snap: when `tempo_sync && beat_phase < last_beat_phase - 0.5 && division ≤ 1 beat`, reset `phase = 0.0`. Port logic from `Lfo::update()`. | `rustjay-core/src/modulation.rs` | Unit test: simulate beat_phase wrap; phase snaps to 0. |
| **M1.3a** Add `last_beat_phase: f32` as a `#[serde(skip)]` runtime field to `ModulationSource::LFO`. This is the missing storage for the snap guard: the current `Lfo` struct stores it on the struct itself; in the new enum variant it must be an inline field. Without this, M1.3 has nowhere to persist the previous beat phase. | `rustjay-core/src/modulation.rs` | `ModulationSource::LFO { ..., last_beat_phase: f32 }` compiles; round-trip serde does not persist it (runtime only). |
| **M1.4** Change `ModulationEngine::update(&mut self, time: f32, bpm: f32, beat_phase: f32, audio: &AudioValues)`. | `rustjay-core/src/modulation.rs` | All callers compile after signature change. |
| **M1.4a** Thread BPM and beat_phase from the active sync source into `update()`. Add `EngineState::effective_bpm() -> f32` and `EngineState::effective_beat_phase() -> f32` helpers that return: Link values when `SyncSource::AbletonLink` and `link.enabled`, ProDJ values when `SyncSource::ProDj` and `prodj.enabled`, else `audio.bpm` / `audio.beat_phase`. The `update_lfo()` call site uses these instead of reading `audio.*` directly. | `rustjay-core/src/state.rs` | At 120 BPM via Link, tempo-sync LFOs run at Link tempo, not audio-detected BPM. |
| **M1.5** Add `LFOWaveform::Random` if not present; map old `Waveform::Ramp` and `Waveform::Saw` to `LFOWaveform::Sawtooth` in adapters. | `rustjay-core/src/modulation.rs` | Waveform count ≥ 5. |

> **Caller inventory for M1.4** — before touching the signature, confirm every call site:
> - `rustjay-engine/src/app/update.rs` → `App::update_lfo()` (primary tick path)
> - `examples/varda/src/lib.rs` → `VardaPlugin::update()` (owns a separate `ModulationEngine` today; removed in Phase 4)
> - `rustjay-mixer/src/lib.rs` → `Mixer::render_to()` (owns a separate `ModulationEngine` today; removed in Phase 4)
> - Any test that calls `engine.update(time, audio)` directly — update those signatures in this phase so CI stays green.

**Gate:** `cargo test -p rustjay-core` green, clippy clean.

---

### Phase 2 — `EngineState` hosts the unified engine *(Medium risk, 2–3 days)*

Move ownership to `EngineState`. Keep a backward-compatible shim so `waaaves`/`delta` don't break immediately.

| Task | File | Acceptance |
|---|---|---|
| **M2.0** Serialization. `EngineState` has no `Serialize`/`Deserialize` derive; presets use the intermediary `Preset` struct. `Preset` clones the inner `ModulationEngine` from `state.modulation.lock()` and serializes that directly (Option A from A.2). No newtype wrapper needed. | `rustjay-presets/src/presets.rs` | `Preset::from_state` and `apply_to_state` round-trip `ModulationEngine` sources + assignments. |
| **M2.1** Add `pub modulation: Arc<Mutex<ModulationEngine>>` to `EngineState`. Initialize with 8 default LFO sources (mimicking `LfoBank::new()`) so existing presets expecting 8 slots find them. | `rustjay-core/src/state.rs` | `EngineState::new()` compiles. |
| **M2.2** Update `App::update_lfo()` to tick `state.modulation` instead of `state.lfo.bank`. Apply HSB offsets by reading `"hue_shift"`, `"saturation"`, `"brightness"` from the modulation engine. | `rustjay-engine/src/app/update.rs` | HSB modulation still works in `delta`. |
| **M2.3** Update `App::update_lfo()` custom-param path: **stop** pre-mutating `custom_params`. Instead, rely on `get_param()` to add modulation on demand. | `rustjay-engine/src/app/update.rs` | `get_param("custom_0")` returns base + modulation. |
| **M2.4** Keep `pub lfo: LfoState` on `EngineState` as a **deprecated shim** for one release. Implement `Deref` or accessor methods that read through to `modulation` so `waaaves/src/lfo_ui.rs` compiles with minimal changes. Mark with `#[deprecated]`. | `rustjay-core/src/state.rs`, `rustjay-core/src/lfo.rs` | `cargo build -p waaaves` green. |
| **M2.5** Serialization: `EngineState` no longer serializes `lfo` directly; presets save/restore `modulation` sources + assignments. Add `LfoBank::to_modulation_engine()` migration for old preset JSON. | `rustjay-core/src/state.rs`, `rustjay-core/src/lfo.rs` | Loading an old preset with `LfoState` JSON populates the new `modulation` engine. |

> **Lock-hierarchy note (prevents deadlock).** `EngineState` is already wrapped in `Arc<Mutex<EngineState>>` (`shared_state`). If `modulation` is a *nested* `Arc<Mutex<ModulationEngine>>` inside `EngineState`, the engine tick acquires `shared_state.lock()` first and then `modulation.lock()`. The GUI thread must always acquire them in the same order. The safest pattern: make the GUI write path go through a `ModulationCommand` enum dispatched in `dispatch_commands()` (consistent with every other subsystem in `commands.rs`), so the GUI never needs `shared_state` locked to write to modulation. If direct GUI access to the modulation arc is required (e.g., for previewing LFO values in real time), expose a separately-cloned `Arc<Mutex<ModulationEngine>>` that the GUI can lock independently — it must never also hold `shared_state` at that point.

**Gate:** `cargo build --workspace` green; `cargo test -p rustjay-core` green; `delta` and `waaaves` launch without regressions.

---

### Phase 3 — `get_param()` becomes modulation-aware *(Medium risk, 2 days)*

This is the critical behavioral shift: every param read goes through the unified engine.

| Task | File | Acceptance |
|---|---|---|
| **M3.1** Implement `EngineState::get_param_modulated(id: &str) -> Option<f32>` that looks up base + modulation offset. | `rustjay-core/src/state.rs` | Unit test: base=0.5, modulation=+0.1 → returns 0.6, clamped to descriptor min/max. |
| **M3.1a** Define `get_param_base()` contract: for known engine-owned params (`hue_shift`, `saturation`, `brightness`) return the `hsb_param_bases` value; for `custom_*` return `custom_param_bases[i]`; for unrecognised keys return `None`. Document this explicitly so mixer/deck params (e.g., `crossfader`) work correctly once Phase 4 wires them up. | `rustjay-core/src/state.rs` | `get_param_base("crossfader")` returns the pre-modulation value when the mixer registers the param descriptor. |
| **M3.2** Redirect `EngineState::get_param()` to the modulated path for known keys (`custom_*`, `hue_shift`, `saturation`, `brightness`). Keep `get_param_base()` for the raw value. | `rustjay-core/src/state.rs` | `get_param("hue_shift")` includes LFO offset; `get_param_base("hue_shift")` does not. |
| **M3.2a** Hot-path locking: `get_param()` must not re-lock `modulation` on every call — in a single frame `build_uniforms` may call it dozens of times. At the top of each engine tick (after `ModulationEngine::update()`), copy every assigned param's total modulation offset into a `HashMap<String, f32>` on `EngineState` (`modulation_offsets: HashMap<String, f32>`) that `get_param()` reads without a lock. Update the map under the existing `modulation.lock()` already held for `update()`. This is the concrete form of R1's "cache" mitigation. | `rustjay-core/src/state.rs`, `rustjay-engine/src/app/update.rs` | `get_param()` acquires zero mutexes; offsets are updated once per frame. |
| **M3.3** Ensure HSB params are registered as implicit modulation targets. On init, the engine creates assignments for any source targeting `"hue_shift"` etc. | `rustjay-engine/src/app/update.rs` | Built-in LFO tab targeting HueShift still works. |
| **M3.3a** Plugin param registration: when `App::init()` calls `plugin.parameters()`, register the returned `ParameterDescriptor` ids in the modulation engine so the Modulation tab can list them as assignable targets. Add `EngineState::register_param_descriptors(descs: &[ParameterDescriptor])` that stores the ids in a `Vec<String>` (no `Arc`, no locking). The Modulation tab reads this list to populate the target picker (M5.2/M5.5). Call it again on `HotReload` or whenever the plugin's descriptor list changes. | `rustjay-core/src/state.rs`, `rustjay-engine/src/app/init.rs` | Modulation tab shows `"spin"` and `"scale"` as assignable targets after loading the sputnik plugin. |
| **M3.4** Verify `delta`, `sputnik`, and `waaaves` plugins still read correct values. These apps call `engine.get_param()` in `build_uniforms`. | — | Visual regression: orbits spin, LFO dots appear, HSB shifts color. |

**Gate:** Visual smoke-test on `delta`, `sputnik`, `waaaves`.

---

### Phase 4 — `rustjay-mixer` surrenders its modulation engine *(Medium risk, 2–3 days)*

The mixer stops being a special case.

| Task | File | Acceptance |
|---|---|---|
| **M4.1** Remove `pub modulation: Arc<Mutex<ModulationEngine>>` from `Mixer`. | `rustjay-mixer/src/lib.rs` | Compiles. |
| **M4.2** Update `Mixer::render_to()` to read modulated values via `engine.get_param()` only. Remove all `self.modulation.lock()` calls and manual offset addition. | `rustjay-mixer/src/lib.rs` | `cargo test -p rustjay-mixer` green (21 tests). |
| **M4.3** Update `Mixer::parameters()` to expose the same param ids as before (`crossfader`, `ch_*_opacity`, etc.). The unified engine targets them automatically. | `rustjay-mixer/src/lib.rs` | MIDI/OSC/LFO can target mixer params. |
| **M4.4** Update `MixerState` preset serialization: instead of saving a full `ModulationEngine`, save only the sources + assignments that target mixer param prefixes. On restore, merge into `engine.modulation`. | `rustjay-mixer/src/preset.rs` | Preset round-trip test passes. |
| **M4.5** Remove `modulation` from `MixerState` JSON; bump `MIXER_STATE_VERSION`. | `rustjay-mixer/src/preset.rs` | Old mixer presets without modulation still load; new presets omit redundant engine state. |

**Gate:** `cargo test -p rustjay-mixer` green; `examples/mixer` runs; crossfader + channel opacity modulation observable.

---

### Phase 5 — Built-in GUI tabs become Modulation editors *(Medium risk, 3–4 days)*

Replace the 8-slot LFO tab with an editor for the shared `ModulationEngine`.

| Task | File | Acceptance |
|---|---|---|
| **M5.1** Rename built-in tab enum from `Lfo` → `Modulation`. | `rustjay-core/src/state.rs` (`GuiTab::Lfo` → `GuiTab::Modulation`), `rustjay-gui` tab registry and match arms | Appears in sidebar as "Modulation". |
| **M5.2** Build egui Modulation tab: source list (add/remove), per-source config panel, assignment list (`param_id` + `amount`). | `rustjay-gui/src/egui_tabs/lfo.rs` | Can add an LFO, assign it to `custom_0`, and see the slider move. |
| **M5.3** Build imgui Modulation tab (for `waaaves` / non-egui builds). | `rustjay-gui/src/tabs/tab_lfo.rs` | Same functionality as egui version. |
| **M5.4** Right-click param-slider context menu: "Modulate with…" → pick source + amount. | `rustjay-gui` param widgets | Clicking a slider shows available sources; selecting one creates an assignment. |
| **M5.5** Wire HSB quick-targets: source config panel shows `"hue_shift"`, `"saturation"`, `"brightness"` as target options alongside custom params. | `rustjay-gui/src/egui_tabs/lfo.rs` | LFO can target HSB directly. |
| **M5.6** Deprecation shims: old `LfoBank`-based tab code moves to a `legacy_lfo_tab` module, gated behind a compile flag, and is removed in a follow-up. | `rustjay-gui` | `waaaves` can still compile if it hasn't migrated its own UI yet. |

**Gate:** `cargo build --workspace --all-features` green; `cargo build -p waaaves` green; clicking the Modulation tab does not panic.

---

### Phase 6 — Example migration *(Low risk, 1–2 days)*

Update every example to use the unified system. Most need only minor changes.

| Task | File | Acceptance |
|---|---|---|
| **M6.1** `examples/waaaves/src/lfo_ui.rs` — replace `LfoBank`/`LfoTarget` imports with `ModulationEngine` reads. Draw dots by scanning `engine.modulation.sources`. | `examples/waaaves/src/lfo_ui.rs` | Right-click assignment still works; dots render. |
| **M6.2** `examples/varda/src/ui/mod.rs` — change `ModulationTab` from **read-only** info panel to **read-write** editor. Add/remove sources, assign to params, trigger ADSR. | `examples/varda/src/ui/mod.rs` | Can create an LFO and assign it to a deck opacity from the Varda UI. |
| **M6.3** `examples/varda/src/lib.rs` — remove manual demo-source creation (the 4 demo sources added in `VardaPlugin::init`). Instead, ship a default preset that loads into the shared engine. | `examples/varda/src/lib.rs` | `cargo run -p varda` still shows modulation demo. |
| **M6.4** `examples/mixer/src/main.rs` — remove any manual `mixer.modulation` setup. | `examples/mixer/src/main.rs` | Compiles; built-in Modulation tab drives mixer params. |
| **M6.5** `examples/delta-egui/src/main.rs` — verify no direct LFO imports; confirm `get_param` reads modulated values. | — | No code change expected. |

**Gate:** `cargo build --workspace` green; `cargo test -p varda` green.

---

### Phase 7 — Cleanup & deletion *(Low risk, 1 day)*

Remove deprecated shims once the above is stable.

| Task | File | Acceptance |
|---|---|---|
| **M7.1** Delete `LfoState`, `LfoBank`, `Lfo`, `LfoTarget`, `Waveform` from `rustjay-core/src/lfo.rs`. Remove the file if empty. | `rustjay-core/src/lfo.rs` | Nothing references these types. |
| **M7.2** Remove `pub lfo: LfoState` from `EngineState`. | `rustjay-core/src/state.rs` | `cargo build --workspace` green. |
| **M7.3** Remove legacy LFO tab code from `rustjay-gui`. | `rustjay-gui/src/egui_tabs/lfo.rs` (legacy), `tabs/tab_lfo.rs` (legacy) | Only unified Modulation tab remains. |
| **M7.4** Remove `LfoBank`/`LfoTarget`/`Waveform` re-exports from `rustjay-engine` prelude. | `rustjay-engine/src/lib.rs` | `cargo build --workspace` green. |
| **M7.5** Update guide docs (`guide/src/modulation/*.md`) to describe the unified system. | `guide/src/modulation/lfo.md`, `tempo-sync.md`, `routing.md` | Docs match code. |

**Gate:** `cargo build --workspace --all-features` green; clippy clean; docs build.

---

## 5. API Contract — How Apps Use the Unified System

### Declaring modulatable params (unchanged)

```rust
fn parameters(&self) -> Vec<ParameterDescriptor> {
    vec![
        ParameterDescriptor::float("spin", "Spin", category, 0.0, 360.0, 0.0, 1.0),
    ]
}
```

### Reading modulated values (unchanged call site, new behavior)

```rust
let spin = engine.get_param("spin").unwrap_or(0.0);
// Returns base + modulation_offset, clamped to descriptor min/max.
```

### Programmatic control

```rust
// Add a tempo-sync LFO and assign it to a param
let lfo = {
    let mut eng = engine.modulation.lock().unwrap();
    let uuid = eng.add_source(ModulationSource::LFO {
        waveform: LFOWaveform::Sine,
        tempo_sync: true,
        division: 2,               // 1/4 note
        amplitude: 0.5,
        bipolar: true,
        frequency: 1.0,            // ignored when tempo_sync=true
        phase: 0.0,
        phase_offset_degrees: 0.0,
        last_beat_phase: 0.0,
    });
    eng.assign("spin", &uuid, 1.0, None);
    uuid
};

// Trigger an ADSR envelope
{
    let mut eng = engine.modulation.lock().unwrap();
    eng.trigger_adsr(&adsr_uuid);
}
```

### Custom GUI tabs

```rust
fn ui(&mut self, ui: &mut egui::Ui, engine: &mut EngineState) {
    let mut eng = engine.modulation.lock().unwrap();
    for entry in &eng.sources {
        ui.label(format!("{} — {:?}", entry.uuid, entry.source));
    }
}
```

---

## 6. Architecture Notes

These are design decisions that must be resolved before or during Phase 1–2 to avoid costly rework later.

### A.1 — Runtime state in `ModulationSource::LFO`

The old `Lfo` struct stores `phase`, `output`, and `last_beat_phase` as mutable runtime fields alongside configuration. In the new `ModulationSource::LFO` enum variant, `last_beat_phase` is tagged `#[serde(skip)]` because it is pure runtime state. `phase` is serialized so that presets preserve the user's LFO phase position (e.g. a deliberately offset LFO does not reset to 0 on reload). Full field set:

```rust
ModulationSource::LFO {
    // --- config (serialized) ---
    waveform: LFOWaveform,
    frequency: f32,
    phase: f32,
    amplitude: f32,
    bipolar: bool,
    tempo_sync: bool,
    division: usize,
    phase_offset_degrees: f32,
    // --- runtime (not serialized) ---
    #[serde(skip)]
    last_beat_phase: f32,
}
```

`calculate()` becomes a pure function of these fields + `(bpm, beat_phase, dt)` and mutates `phase` and `last_beat_phase` in place. No separate `prev_values` vector involvement needed for the LFO case.

### A.2 — Serialization of `Arc<Mutex<ModulationEngine>>`

`Arc<Mutex<T>>` does not implement `Serialize`. Two options:

**Option A (recommended):** Store `ModulationEngine` as a plain field on a private serialization-only mirror struct, and keep `Arc<Mutex<ModulationEngine>>` as the live runtime field. In `EngineState`'s custom `Serialize` impl (or the preset save path), lock the arc, clone the inner engine, serialize the clone.

```rust
// In preset save:
let snap = state.modulation.lock().unwrap().clone();
json["modulation"] = serde_json::to_value(&snap)?;
```

**Option B:** Implement a `ModulationEngineCell` newtype with manual serde that calls `lock().unwrap()` inside `serialize`. Ergonomic at the field level but hides the lock and can deadlock if the caller already holds the lock.

Option A is preferred because the lock scope is explicit and the preset path already has full ownership of `EngineState`.

### A.3 — Effective BPM helper

Multiple sync sources (`SyncSource::Audio`, `AbletonLink`, `ProDj`) each publish BPM and beat_phase. All modulation tempo-sync paths must read the same source. Add to `EngineState`:

```rust
pub fn effective_bpm(&self) -> f32 {
    match self.sync_source {
        SyncSource::AbletonLink if self.link.bpm > 0.0 => self.link.bpm,
        SyncSource::ProDj if self.prodj.master_bpm > 0.0 => self.prodj.master_bpm,
        _ => self.audio.bpm,
    }
}

pub fn effective_beat_phase(&self) -> f32 {
    match self.sync_source {
        SyncSource::AbletonLink if self.link.bpm > 0.0 => self.link.beat_phase,
        SyncSource::ProDj if self.prodj.master_bpm > 0.0 => self.prodj.master_beat_phase,
        _ => self.audio.beat_phase,
    }
}

/// Beat phase safe for LFO beat-snap. Returns `0.0` when the active sync source
/// is `Audio` so the audio beat detector's irregular resets do not fire snap logic.
pub fn stable_beat_phase(&self) -> f32 {
    match self.sync_source {
        SyncSource::AbletonLink if self.link.bpm > 0.0 => self.link.beat_phase,
        SyncSource::ProDj if self.prodj.master_bpm > 0.0 => self.prodj.master_beat_phase,
        _ => 0.0,
    }
}
```

`effective_bpm()` and `effective_beat_phase()` replace direct `self.audio.bpm` reads in `update_lfo()`. `stable_beat_phase()` is used by tempo-sync LFO snap logic (M1.3) to avoid irregular phase resets when the audio beat detector is the sync source.

### A.4 — Concurrency model and lock ordering

The engine has two shared resources:
- `shared_state: Arc<Mutex<EngineState>>` — the engine tick holds this for the entire frame
- `modulation: Arc<Mutex<ModulationEngine>>` — nested inside `EngineState`

**Required invariant:** any thread that needs both locks must acquire them in the order `shared_state → modulation`. Violating this causes deadlock.

**Enforcement strategy:**
- The engine tick acquires `shared_state` at the top of the frame, then locks `modulation` for `update()`, writes `modulation_offsets` and `lfo` shim outputs, and releases `modulation` — all within the same lock scope. This is the only place both are held simultaneously.
- The GUI thread writes to modulation via `ModulationCommand` (added to the command enum in Phase 2), dispatched by `dispatch_commands()` inside the engine tick, which already holds `shared_state`. No GUI code locks `modulation` directly for writes.
- For GUI real-time reads (e.g., animated LFO dots in the Modulation tab), the GUI holds a cloned `Arc<Mutex<ModulationEngine>>` and locks it only when `shared_state` is NOT locked. Access `engine.current_values()` (which returns the per-source computed values) rather than calling `update()`.

### A.5 — Param descriptor registration for Modulation tab target picker

The Modulation tab (M5.2) needs a list of all assignable param IDs at render time. This cannot come from `ParameterDescriptor::all()` (no such API) — it must be pushed into `EngineState` when the plugin loads.

Add to `EngineState`:

```rust
/// Flat list of plugin-declared parameter ids; updated on plugin load/reload.
pub registered_param_ids: Vec<String>,
```

Populate in `App::init()` after `plugin.parameters()` returns:

```rust
state.registered_param_ids = plugin.parameters().into_iter().map(|d| d.id).collect();
```

The Modulation tab iterates `state.registered_param_ids` plus the static HSB ids (`"hue_shift"`, `"saturation"`, `"brightness"`) to build the target picker. This replaces the ad-hoc `LfoTarget::all_for(descriptors)` call in the old LFO tab.

### A.6 — `ModulationCommand` enum (Phase 2 addition)

For consistency with every other subsystem in `commands.rs` and to enforce the lock hierarchy in A.4, add:

```rust
pub enum ModulationCommand {
    #[default]
    None,
    AddSource(ModulationSource),
    AddSourceWithUuid { uuid: String, source: ModulationSource },
    RemoveSource(String),
    Assign { param: String, source_id: String, amount: f32, component: Option<usize> },
    AssignModOnMod { target_uuid: String, param: String, modulator_uuid: String, amount: f32 },
    ClearAssignments(String),
    TriggerAdsr(String),
    ReleaseAdsr(String),
    RestoreEngine(ModulationEngine),
}
```

GUI code writes `state.modulation_command = ModulationCommand::AddSource(...)` while holding `shared_state`, or posts it through the normal command-slot mechanism. `dispatch_commands()` processes it at the top of the next frame inside the engine tick.

---

## 7. Serialization & Preset Migration

### Old preset format (LfoBank)

```json
{
  "lfo": {
    "bank": {
      "lfos": [
        { "index": 0, "enabled": true, "target": "Custom", "waveform": "Sine",
          "tempo_sync": true, "division": 2, "rate": 1.0, "phase_offset": 0.0 }
      ]
    }
  }
}
```

### New preset format (ModulationEngine)

```json
{
  "modulation": {
    "sources": [
      { "uuid": "lfo_0", "source": { "LFO": { "waveform": "Sine", "tempo_sync": true,
        "division": 2, "frequency": 1.0, "phase": 0.0, "amplitude": 0.5, "bipolar": true,
        "phase_offset_degrees": 0.0 }}}
    ],
    "assignments": {
      "custom_0": [{ "source_id": "lfo_0", "amount": 1.0, "component": null }]
    }
  }
}
```

### Migration strategy

On preset load, detect the presence of `"lfo"` key. If found:
1. Call `LfoBank::to_modulation_engine()` (already implemented in B2.2 adapter).
2. Store the result in `EngineState.modulation`.
3. Discard the old `lfo` key.
4. Bump the preset version so future saves write the new format.

---

## 7. Risk Register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| **R1** `get_param()` now locks `modulation` on every call — potential deadlock or perf regression in hot render paths. | Medium | High | Addressed by M3.2a: pre-compute total offsets per assigned param into `modulation_offsets: HashMap<String, f32>` once per tick. `get_param()` reads the map with zero mutex acquisitions. |
| **R2** Mixer presets that saved `ModulationEngine` state now need to merge into a shared engine — race conditions if multiple mixers restore presets simultaneously. | Low | Medium | Mixer preset restore takes a write lock on `engine.modulation`, clears orphaned assignments for its own param prefix, then inserts new ones. |
| **R3** `waaaves` right-click LFO assignment UI is tightly coupled to `LfoBank` indices and `LfoTarget`. | Medium | Medium | Phase 2 shim keeps it compiling; Phase 6 migrates it. If shim proves fragile, prioritize M6.1 early. |
| **R4** `EngineState` serialization size grows because `ModulationEngine` sources are unbounded. | Low | Low | Presets already cap sources (`MAX_MOD_SOURCES = 64`) and assignments (`MAX_MOD_ASSIGNMENTS = 256`). Runtime engine has no hard cap but is bounded by UI. |
| **R5** Quantum-boundary snap changes LFO phase behavior slightly vs. old `LfoBank` (different epsilon on wrap detection). | Low | Low | Add a unit test that compares old `Lfo::update()` and new `ModulationSource::LFO` at identical BPM/beat_phase inputs; assert phase equality within 1e-4. |
| **R6** Nested `Arc<Mutex>` deadlock: GUI thread locks `modulation` while engine tick also holds `shared_state` and tries to lock `modulation`. | Low | High | Addressed by lock-hierarchy note in Phase 2: GUI writes go through `ModulationCommand` dispatch; GUI reads use a separately-cloned arc held only when `shared_state` is NOT locked. Verify with `cargo test --features deadlock-detection` (parking_lot) before Phase 3 merges. |
| **R7** ~~`last_beat_phase` omitted from `ModulationSource::LFO` variant — M1.3 snap logic has nowhere to write state.~~ **Resolved.** `last_beat_phase: f32` with `#[serde(skip)]` was added in M1.3a and is live in `modulation.rs`. Quantum-boundary snap unit tests pass. | — | — | Closed 2026-06-06. |

---

## 8. Definition of Done

- [ ] `cargo build --workspace --all-features` green, warning-clean.
- [ ] `cargo test -p rustjay-core` green.
- [ ] `cargo test -p rustjay-mixer` green.
- [ ] `cargo test -p rustjay-gui` green (if tests exist).
- [ ] `cargo run -p delta` — visual output unchanged, HSB + custom param modulation works.
- [ ] `cargo run -p waaaves` — LFO dots and right-click assignment work.
- [ ] `cargo run -p sputnik` — mesh LFO displacement works.
- [ ] `cargo run -p mixer` — crossfader + channel opacity modulation works via built-in Modulation tab.
- [ ] `cargo run -p varda` — ModulationTab is read-write; tempo-sync LFO can be created and assigned to deck opacity.
- [ ] Old preset containing `LfoState` loads correctly and produces identical modulation output.
- [ ] `guide/src/modulation/*.md` updated.
- [ ] `PHASE_B_ROADMAP.md` B2.4 marked complete.
- [ ] `VARDA_PORT.md` T04.1 updated to describe unified `ModulationEngine` with tempo sync.

---

## 9. Changelog (to be filled as work progresses)

- **2026-06-06** — Roadmap drafted.
- **2026-06-06** — Architect review added: M1.3a (`last_beat_phase` field), M1.4a (effective BPM helper), M2.0 (serialization decision), M3.1a (`get_param_base` contract), M3.2a (lock-free snapshot), M3.3a (param registration), lock-hierarchy note in Phase 2, Section 6 architecture notes (A.1–A.6), R6–R7 in risk register.
- **2026-06-06** — Phase 1 (M1.1–M1.5) complete: `ModulationSource::LFO` gains tempo sync, quantum-boundary snap, new `update()` signature, waveform parity. 85 unit tests pass.
- **2026-06-06** — Phase 2 (M2.1–M2.5) complete: `EngineState` hosts unified `ModulationEngine`, `update_lfo()` ticks it, preset migration works, `waaaves` shim compiles. Workspace build green.
