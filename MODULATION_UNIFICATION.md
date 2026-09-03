# Modulation Unification — one engine, one grid, one envelope story

Status: **plan**, not implemented. Decisions taken 2026-09-03.

## 1. Why

The engine runs two modulation systems that do not know about each other:

| | Audio routing matrix | Modulation engine |
|---|---|---|
| Type | `AudioRoutingState` / `RoutingMatrix` | `ModulationEngine` |
| Band | 8 fixed enum bands | arbitrary `freq_low`/`freq_high` |
| Target | fixed `ModulationTarget` enum + `Custom(String)` | any param id, many per source |
| Shape | `amount`, `attack`, `release` | `gain`, `smoothing`, `mode`, `noise_gate` |
| Sources | audio only | LFO, AudioBand, ADSR, StepSequencer |
| Chaining | none | mod-on-mod |

They meet in exactly one place, `EngineState::get_param`, which returns
`routed + offset * range` for a custom param. Audio routing bakes its result
into `custom_params` each frame; the modulation engine adds an offset on top.

**A parameter with both a route and an assignment therefore gets both, summed,
and neither UI shows the other's existence.** That is the bug living in the
overlap, and it is silent.

`docs/archive/UNIFIED_MODULATION_ROADMAP.md` parked this deliberately —
"Audio routing matrix is not absorbed yet — it stays separate but may be
deprecated later". Its Phases 3–4 are done in the code but unticked; Phase 7
cleanup never ran. This plan finishes that migration rather than starting a
new architecture.

## 2. Routing is a strict subset

`RoutingMatrix::to_modulation_engine()` already exists and converts every route
into an `AudioBand` source plus an assignment. Nothing the matrix does is
inexpressible in the modulation engine.

One real capability gap: a route has **separate attack and release**;
`AudioBand` has a single `smoothing`. The existing bridge maps
`smoothing := route.release` and drops attack, so migrating as-is loses feel.

## 3. Decisions

- **The band→target grid stays.** It is the fastest way to set up audio
  reactivity for a show, and much quicker than the per-parameter popup. It
  becomes a *view* over the modulation engine rather than a system that owns
  its own state.
- **Accumulate modes go.** `AudioReactMode::Increase` / `Decrease` integrate
  audio energy into upward or downward drift, wrapping at the ends. They are
  awkward to reason about and overlap badly with a triggered envelope, which
  is the better answer for "audio makes something happen over time".
  `Direct` — a plain envelope follower — is the only mode kept.

  Blast radius is small: the two variants are implemented in
  `modulation.rs` and referenced nowhere else, and no UI exposes the mode
  picker at all. The serde variants must still *deserialise* (old scenes and
  presets may name them) and fold to `Direct` on load.

## 4. Target model

One source of truth: `ModulationEngine`. The per-frame
`RoutingMatrix::apply_to_params` path is deleted, which removes the summing
point in `get_param` along with it.

Three roles, named separately because they are currently conflated:

- **Follow** — `AudioBand`. Continuous; tracks energy in a band. Exists.
- **Trigger** — fires a gate on a transient. **Missing entirely.**
- **Shape** — `ADSR`. A one-shot needing a gate. Exists, but
  `trigger_adsr` has exactly one caller: a programmatic `ModulationCommand`.
  Nothing in the audio path can fire it.

So today "audio reactivity" can only mean *value follows loudness*, never
*a hit fires an envelope* — which is what people usually mean by envelope
following, and the more musical of the two.

The missing piece:

```rust
ModulationSource::AudioTrigger {
    band_low: f32,
    band_high: f32,
    threshold: f32,
    hysteresis: f32,
    min_interval: f32,   // retrigger lockout, seconds
}
```

emitting a gate, plus beat-clock triggers off the existing BPM. The wiring
already exists: `assign_mod_on_mod(adsr_uuid, "gate", trigger_uuid, …)` fits
if the ADSR's gate is treated as a modulatable parameter of the source. No new
mechanism is needed for it.

This also gives the MOD popup a coherent vocabulary: **Follow** a band,
**Trigger** on a band, or **Shape** what a trigger fires.

## 5. Phases

| Phase | Work | Gate |
|---|---|---|
| **U1** | Add `attack` to `AudioBand` beside `smoothing` (rename to `release`), so a route migrates without losing feel. Serde defaults keep old data loading. | Existing audio mods behave as before; a migrated route matches its old attack/release. |
| **U2** | Migrate `AudioRoutingState` on load through `to_modulation_engine()`, as `LfoBank` already does. Keep deserialising the field; do not delete it. | An old scene's routes come back as sources + assignments, and are saved in the new shape. |
| **U3** | Delete the `apply_to_params` per-frame path and the `routed +` term in `get_param`. | No parameter can receive a contribution twice; `get_param` is `base + offsets`. |
| **U4** | Rebuild the routing-matrix window as a grid view over `AudioBand` sources and their assignments. Rows edit sources; the grid gains nothing of its own. | Editing in the grid and in the MOD popup show the same state. |
| **U5** | Fold `Increase`/`Decrease` to `Direct` on load; drop the variants from the live enum. | Old presets load; nothing references the removed variants. |
| **U6** | `ModulationSource::AudioTrigger`, and the ADSR gate as a mod-on-mod target. | A drum hit fires an ADSR bound to a parameter. |

## 6. Compatibility

- `audio_routing` is serialised in KOVVBOJ scenes and `routing_matrix` in
  presets. Both must keep deserialising and migrate on load — the fields
  cannot simply be dropped.
- The 64-route cap becomes moot once routes are sources; sources are already
  unbounded.
- `ModulationTarget::param_id()` already maps the fixed enum to param ids, so
  targets survive the move without a lookup table.

## 7. Already done

- Several sources can drive one parameter from the map popup, which lists the
  stack and removes them individually (`f65cc67`). The engine always summed
  assignments; only that popup replaced them.
