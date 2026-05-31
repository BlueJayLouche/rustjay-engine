# LFOs

rustjay-engine provides **8 LFO slots**. Each slot is an independent oscillator that can be assigned to any declared parameter at runtime — from the Modulation tab (desktop) or the **Modulation** web panel (headless).

## Configuration

Each slot exposes:

| Field | Description |
|---|---|
| **Enabled** | Toggle the slot on/off without losing its settings |
| **Waveform** | Shape of the oscillator (see table below) |
| **Target** | Which parameter this slot drives; built-in or any effect-declared parameter |
| **Depth** | Modulation amplitude — scales the `[-1, 1]` oscillator output |
| **Tempo Sync** | On: rate is expressed as a beat division; Off: rate in Hz |
| **Division** | Beat subdivision when tempo sync is on (1/16 through 8 beats) |
| **Rate (Hz)** | Oscillator frequency when tempo sync is off |
| **Phase Offset** | Starting phase in degrees (0–360) — useful for quadrature pairs |

## Waveforms

| Waveform | Shape | Output range |
|---|---|---|
| `Sine` | Smooth sinusoidal | `[-1, 1]` |
| `Triangle` | Linear V-shape | `[-1, 1]` |
| `Ramp` | Rising linear ramp, instant reset | `[-1, 1]` |
| `Saw` | Falling linear ramp, instant reset | `[-1, 1]` |
| `Square` | Instant high/low | `[-1, 1]` |

Depth scales the `[-1, 1]` oscillator output before it is added to the parameter's base value. A depth of `0.5` means the LFO swings ±0.5 units from whatever the base value is set to.

## Beat-sync mode

When **Tempo Sync** is on, the division field replaces the rate field. The engine converts it using `effective_bpm()`:

```
cycle_duration_seconds = (60 / bpm) * beats_per_cycle
```

At 120 BPM, a `1/4` division (0.25 beats) runs at 8 Hz; a `1` beat division runs at 2 Hz; a `4` beat division runs at 0.5 Hz.

The LFO phase advances freely based on wall-clock delta time — it does **not** directly track `beat_phase`. When using Ableton Link or ProDJ Link, the phase snaps to the quantum boundary on each beat crossing to stay musically in phase. With audio beat detection or tap tempo, the phase resets freely and may drift relative to the actual beat.

## Tap tempo

On headless Pi setups without Ableton Link, use **Tap Tempo** in the Modulation web panel to set the BPM manually. Tap twice for an immediate estimate; subsequent taps refine the average over up to 8 intervals. The BPM display updates after each tap.

Tap tempo writes to `audio.bpm`, which `effective_bpm()` returns when the active sync source is *Audio*.

## Reading LFO values in code

When you call `engine.get_param(id)`, LFO modulation is already included — you don't need to read the LFO state directly.

If you need the raw oscillator output (e.g. to drive something outside the parameter system):

```rust
fn build_uniforms(&self, s: &MyState, engine: &EngineState) -> MyUniforms {
    let lfo_0 = engine.lfo.bank.lfos[0].output; // f32 in [-amplitude, amplitude]
    let lfo_1 = engine.lfo.bank.lfos[1].output;
    MyUniforms {
        wobble: lfo_0,
        // ...
    }
}
```

`output` is in `[-amplitude, amplitude]` — it is the raw waveform value multiplied by depth.

## Phase continuity on config update

When an LFO's configuration is changed via the web Modulation panel, the engine preserves the current `phase`, `output`, and `last_beat_phase` values from the existing slot before applying the new config. A running LFO does not snap back to phase 0 when you adjust its waveform or depth mid-cycle.

## Targeting effect-declared parameters

LFO targets fall into two groups:

**Built-in targets** — `HueShift`, `Saturation`, `Brightness`. These modulate the HSB colour correction layer common to all effects.

**Custom targets** — any parameter declared by the effect via `ParameterDescriptor`. In the web Modulation panel, all current effect parameters appear in the Target dropdown under their category name (e.g. *Flux / Flow Scale*). Internally, these are stored as `LfoTarget::Custom("flow_scale")` using the bare parameter ID — the category prefix is stripped before storage.

## Multiple LFOs on one parameter

If two or more enabled slots target the same parameter, their outputs are summed before being applied. The summed modulation is still clamped to the parameter's `[min, max]` range.
