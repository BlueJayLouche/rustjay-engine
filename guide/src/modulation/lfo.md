# LFOs

rustjay-engine includes three LFO banks (A, B, C). Each bank is an independent oscillator that can be assigned to any declared parameter at runtime from the LFO tab.

## The LFO tab

Each bank shows:
- **Waveform** — Sine, Triangle, Saw, Square, or Noise
- **Rate** — in Hz or as a beat division (1/4, 1/8, 1/16, etc.)
- **Depth** — modulation amount; multiplied by the waveform output
- **Target** — which declared parameter this bank drives

The user assigns LFO banks to parameters at runtime. Your code doesn't need to know which parameter any given bank is targeting.

## Waveforms

| Waveform | Shape | Output range |
|---|---|---|
| `Sine` | Smooth sinusoidal | `[-1, 1]` |
| `Triangle` | Linear V shape | `[-1, 1]` |
| `Saw` | Rising ramp, instant reset | `[-1, 1]` |
| `Square` | Instant high/low | `[-1, 1]` |
| `Noise` | Random per-sample | `[-1, 1]` |

Depth scales the `[-1, 1]` oscillator output before it's added to the base parameter value. A depth of 0.5 means the LFO swings ±0.5 units from the base.

## Beat-sync mode

When **Beat Sync** is enabled on a bank, its rate is expressed as a beat division rather than Hz. The engine converts it using the current effective BPM:

```
hz = (bpm / 60.0) * (1.0 / beat_division)
```

So at 120 BPM, a `1/4` note rate runs at 2 Hz; a `1/2` note runs at 1 Hz.

The LFO phase locks to `engine.effective_beat_phase()`, so it stays in sync even when the tempo source changes. This is the recommended way to drive beat-locked visuals.

## Reading LFO values in code

When you use `engine.get_param(id)`, LFO modulation is already included in the returned value. You rarely need to read raw LFO state directly.

If you do need the raw oscillator output (e.g. to drive something that isn't a parameter):

```rust
fn build_uniforms(&self, s: &MyState, engine: &EngineState) -> MyUniforms {
    // Direct read — no parameter system involved
    let lfo_a = engine.lfo.banks[0].current_value; // f32 in [-1, 1]
    let lfo_b = engine.lfo.banks[1].current_value;
    MyUniforms {
        wobble: lfo_a * s.wobble_depth,
        // ...
    }
}
```

## Driving multiple parameters

Each bank targets one parameter. To modulate multiple parameters with different shapes, assign each to a different bank:

- Bank A → hue_shift (Sine, 0.25 Hz)
- Bank B → saturation (Triangle, beat-sync 1/2)
- Bank C → brightness (Noise, 4 Hz)

## Phase offset

The Saw and Sine waveforms respect a phase offset that can be set programmatically if you're building a custom LFO UI. A phase of `0.5` starts the waveform halfway through its cycle — useful for creating quadrature pairs.
