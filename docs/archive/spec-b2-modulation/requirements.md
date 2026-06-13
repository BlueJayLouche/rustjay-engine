# B2 — rustjay-modulation

**Version:** 1.0
**Source:** Port of Varda's UUID-stable modulation engine into `rustjay-core`.
**Roadmap:** PHASE_B_ROADMAP.md Phase **B2** (parallel with B0/B1, unblocks B3 T13).
**Status:** Draft

---

## REQ-01 — UUID-Stable Sources

### REQ-01.1 — Source identity
THE SYSTEM SHALL represent every modulation source (LFO, audio band, ADSR, step
sequencer) as a `ModulationSourceEntry` pairing a `ModulationSource` with a stable
UUID string.

### REQ-01.2 — UUID persistence
WHEN a `ModulationEngine` is serialized to JSON and deserialized
THE SYSTEM SHALL preserve source UUIDs so that assignments remain valid across
save/load round-trips.

### REQ-01.3 — ID generation
THE SYSTEM SHALL generate 8-character hex UUIDs for new sources using a
cryptographically-random v4 UUID (identical to Varda's `generate_short_uuid`).

---

## REQ-02 — Assignment Model

### REQ-02.1 — Multi-target per source
THE SYSTEM SHALL allow a single source to modulate any number of parameters via a
`HashMap<String, Vec<ParamModulation>>` assignment table.

### REQ-02.2 — Component modulation
THE SYSTEM SHALL support per-component modulation for vector/color parameters via
an optional `component: Option<usize>` field in `ParamModulation`.

### REQ-02.3 — Mod-on-mod
THE SYSTEM SHALL support modulation-of-modulation: a source may modulate the
parameters (frequency, amplitude, attack, etc.) of another source. Keys SHALL use
the format `mod:{target_uuid}:{param}`.

### REQ-02.4 — Evaluation order
THE SYSTEM SHALL evaluate sources in dependency order so that mod-on-mod
assignments read correct values. Cycles SHALL be broken by a max-depth fallback
(4 passes) with remaining sources appended in index order.

---

## REQ-03 — Source Types

### REQ-03.1 — LFO
THE SYSTEM SHALL provide a low-frequency oscillator with waveforms Sine, Square,
Triangle, Sawtooth, and Random. Output SHALL be in [0, 1] for unipolar mode and
[-1, 1] for bipolar mode, scaled by `amplitude`.

### REQ-03.2 — Audio band
THE SYSTEM SHALL provide an audio-reactive source that computes energy in a
frequency range from FFT data using a dB-based perceptual mapping. It SHALL
support Direct, Increase, and Decrease react modes with adjustable smoothing and
noise gate.

### REQ-03.3 — ADSR envelope
THE SYSTEM SHALL provide an attack-decay-sustain-release envelope with gate
on/off triggers and stage progression (Idle → Attack → Decay → Sustain → Release
→ Idle).

### REQ-03.4 — Step sequencer
THE SYSTEM SHALL provide a step sequencer with configurable step count, rate,
interpolation (None / Linear / Smooth), and bipolar mode.

---

## REQ-04 — Engine Tick

### REQ-04.1 — O(1) source lookup
THE SYSTEM SHALL cache `uuid → index` in a `HashMap` rebuilt only on structural
changes (add/remove source). The `update()` hot path SHALL use this cache, not a
linear scan.

### REQ-04.2 — No per-frame allocation
THE `update()` method SHALL NOT allocate heap memory after the first call. Value
vectors (`prev_values`, `current_values`) SHALL be pre-allocated and grown only
when sources are added.

### REQ-04.3 — Frame inputs
THE `update(time, audio)` signature SHALL accept a monotonic time in seconds and
an `AudioValues` struct carrying per-source FFT/level data.

---

## REQ-05 — Backward Compatibility

### REQ-05.1 — Old API intact
THE SYSTEM SHALL keep `LfoBank`, `Lfo`, `LfoTarget`, `Waveform`, `RoutingMatrix`,
`AudioRoute`, `ModulationTarget`, `FftBand`, `AudioRoutingState` and all their
public methods unchanged and compiling. `waaaves` SHALL continue to use its 8
LFOs without modification.

### REQ-05.2 — Adapter methods
THE SYSTEM SHALL provide `LfoBank::to_modulation_engine()` and
`RoutingMatrix::to_modulation_engine()` adapter methods that convert legacy state
into the new `ModulationEngine` model.

### REQ-05.3 — Deprecation (soft)
Old modulation types SHOULD carry `#[deprecated]` once all in-tree consumers have
migrated. For this phase, deprecation is **not** required — only preservation.

---

## REQ-06 — Tests

### REQ-06.1 — Serialize round-trip
A unit test SHALL create an engine with multiple sources and assignments,
serialize to JSON, deserialize, and assert UUIDs and configurations are equal.

### REQ-06.2 — O(1) path
A unit test SHALL assert that `update()` on an engine with 16 sources and 32
assignments does not reallocate value vectors after the first tick.

### REQ-06.3 — Source value ranges
Unit tests SHALL verify LFO waveforms stay in range, ADSR stage transitions work,
step sequencer interpolation is correct, and audio band energy mapping behaves
for empty/silent/loud signals.

### REQ-06.4 — Mod-on-mod
Unit tests SHALL verify evaluation order respects dependencies, cycles do not
hang, and deep chains (≥4 levels) fall back gracefully.

---

## REQ-07 — Integration

### REQ-07.1 — rustjay-core location
All new modulation vocabulary SHALL live in `rustjay-core/src/modulation.rs` and
be exported from `rustjay-core/src/lib.rs` so any crate in the workspace can use
it without adding new dependencies.

### REQ-07.2 — No workspace breakage
`cargo check --workspace`, `cargo test --workspace`, `cargo build -p delta`, and
`cargo build -p waaaves` SHALL all succeed without modification to existing code.
