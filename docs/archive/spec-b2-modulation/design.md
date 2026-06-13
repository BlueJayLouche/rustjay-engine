# B2 — Design

**Version:** 1.0
**Status:** Draft
**Builds on:** Existing `LfoBank` + `RoutingMatrix` in `rustjay-core`.

---

## 1. Module Layout

```
rustjay-core/src/
  lib.rs            ← pub mod modulation; + re-exports
  modulation.rs     ← new: engine, sources, audio types, tests
  lfo.rs            ← unchanged public API; + adapter methods
  routing.rs        ← unchanged public API; + adapter methods
```

No new crate is created for B2. The modulation vocabulary lands directly in
`rustjay-core` because it is shared state that `rustjay-mixer`, `rustjay-gui`,
`rustjay-presets`, and `rustjay-engine` all need to reference. A future
`rustjay-modulation` leaf crate (GUI widgets, API routes) may depend on core and
extend this vocabulary.

---

## 2. Type Reference (ported from Varda)

### 2.1 Enums

| Type | Source file | Notes |
|---|---|---|
| `LFOWaveform` | `modulation.rs` | Sine, Square, Triangle, Sawtooth, Random |
| `AudioReactMode` | `modulation.rs` | Direct, Increase, Decrease |
| `AudioBandPreset` | `modulation.rs` | Low, Mid, High, Full — convenience for UI |
| `ADSRStage` | `modulation.rs` | Idle, Attack, Decay, Sustain, Release |
| `StepInterpolation` | `modulation.rs` | None, Linear, Smooth |

### 2.2 Structs

| Type | Fields | Serialize? |
|---|---|---|
| `ParamModulation` | `source_id: String`, `amount: f32`, `component: Option<usize>` | Yes |
| `ModulationSourceEntry` | `uuid: String`, `source: ModulationSource` | Yes |
| `AudioSourceValues` | `fft: Vec<f32>`, `level: f32`, `sample_rate: f32` | No (frame data) |
| `AudioValues` | `sources: HashMap<u32, AudioSourceValues>` | No (frame data) |
| `ModulationEngine` | `sources`, `assignments`, `uuid_to_idx`, `prev_values`, `current_values`, `prev_time` | Partial (skip caches) |

### 2.3 Source Variant Mapping

```rust
pub enum ModulationSource {
    LFO { waveform, frequency, phase, amplitude, bipolar },
    AudioBand { source_id: Option<u32>, freq_low, freq_high, gain, smoothing, mode, noise_gate },
    ADSR { attack, decay, sustain, release, stage, stage_time, gate, current_level },
    StepSequencer { steps, rate, interpolation, bipolar },
}
```

`AudioSourceId` from Varda becomes plain `u32` to avoid adding a newtype that
would require an `audio` module dependency.

---

## 3. Engine Lifecycle

```
new()
  └─ empty vectors, empty HashMaps

add_source(source) → uuid
  └─ push entry, push 0.0 to value vecs, insert into uuid_to_idx

remove_source(uuid)
  └─ remove entry + values, clean assignments, rebuild uuid_to_idx

assign(param, source_id, amount, component)
  └─ validate source_id exists (or exists after ensure_index)

update(time, audio)
  ├─ ensure_index (noop if already synced)
  ├─ compute dt
  ├─ grow value vecs if sources grew
  ├─ evaluation_order() → topo-sorted indices
  │   └─ build dep graph from mod:{} keys, 4-pass Kahn-like
  ├─ for i in order:
  │     apply_mod_on_mod(i) → effective source
  │     effective.calculate(time, dt, audio, prev_values[i]) → value
  │     copy back ADSR mutable state
  │     current_values[i] = value
  │     prev_values[i] = value
  └─ done

get_modulation(param) → f32
  └─ sum assignments[param] × current_values[idx] × amount
```

---

## 4. Adapter Design (B2.2)

### 4.1 LfoBank → ModulationEngine

```rust
impl LfoBank {
    pub fn to_modulation_sources(&self) -> Vec<ModulationSourceEntry> { ... }
    pub fn to_modulation_engine(&self, bpm: f32) -> ModulationEngine { ... }
}
```

Mapping rules:
- Each enabled LFO with a non-None target becomes one `ModulationSourceEntry`.
- `Waveform` → `LFOWaveform`:
  - Sine → Sine
  - Triangle → Triangle
  - Square → Square
  - Ramp → Sawtooth (upward ramp)
  - Saw → Sawtooth (downward saw mapped to same; phase inversion accepted)
- `tempo_sync` → `frequency = beat_division_to_hz(division, bpm)` if true, else `rate`.
- `phase_offset` degrees → normalized `phase = phase_offset / 360.0`.
- `amplitude` copied directly.
- `bipolar` always `true` (core LFOs are bipolar [-1, 1]).
- Assignments are created from `target.param_id()` → source UUID with `amount = 1.0`.

### 4.2 RoutingMatrix → ModulationEngine

```rust
impl RoutingMatrix {
    pub fn to_modulation_sources(&self) -> Vec<ModulationSourceEntry> { ... }
    pub fn to_modulation_engine(&self) -> ModulationEngine { ... }
}
```

Mapping rules:
- Each enabled route becomes one `ModulationSourceEntry` of type `AudioBand`.
- `FftBand` → `(freq_low, freq_high)` via hard-coded ranges matching the band definitions.
- `amount` becomes `gain`.
- `attack`/`release` smoothing from the old model maps to a single `smoothing` value
  (average of attack and release for the adapter).
- Assignments are created from `target.param_id()` → source UUID with `amount = 1.0`.

---

## 5. Serialization Format

Example JSON (pretty-printed):

```json
{
  "sources": [
    {
      "uuid": "a1b2c3d4",
      "source": {
        "LFO": {
          "waveform": "Sine",
          "frequency": 1.0,
          "phase": 0.0,
          "amplitude": 0.5,
          "bipolar": true
        }
      }
    }
  ],
  "assignments": {
    "brightness": [
      { "source_id": "a1b2c3d4", "amount": 1.0, "component": null }
    ]
  }
}
```

`uuid_to_idx`, `prev_values`, `current_values`, `prev_time` are `#[serde(skip)]`.

---

## 6. Performance Budget

| Metric | Target | How |
|---|---|---|
| `update()` alloc | 0 after first call | Pre-grow `prev_values` / `current_values` |
| Source lookup | O(1) | `uuid_to_idx` HashMap |
| Evaluation order | O(n + m) | n = sources, m = mod-on-mod edges; cached until assignments change |
| `get_modulation()` | O(k) | k = assignments for that param only |
