# Audio Analysis

rustjay-engine captures audio from the system default input and analyses it every frame. The results are available in `EngineState::audio` from `build_uniforms()` and `prepare()`.

## The audio state

```rust
pub struct AudioState {
    pub volume:      f32,       // RMS loudness, normalised [0, 1]
    pub fft:         [f32; 8],  // 8-band spectrum, [0, 1] each
    pub beat_pulse:  bool,      // true on beat onset this frame
    pub bpm:         f32,       // estimated BPM
    pub beat_phase:  f32,       // position within current beat [0, 1)
}
```

### Volume

`volume` tracks the RMS loudness of the current audio frame, normalised to `[0, 1]`. It responds quickly — useful for direct amplitude-reactive effects.

### FFT bands

`fft` splits the audio spectrum into 8 bands from bass to treble. Band 0 captures the lowest frequencies (sub-bass / kick drum energy), band 7 captures the highest (hi-hat / air).

```rust
let bass   = engine.audio.fft[0]; // kick, sub-bass
let mid    = engine.audio.fft[3]; // midrange, snare
let treble = engine.audio.fft[7]; // hi-hat, brightness
```

All band values are normalised to `[0, 1]`.

### Beat detection

`beat_pulse` is `true` for exactly one frame when a beat onset is detected. Use it to trigger instantaneous events:

```rust
fn build_uniforms(&self, s: &MyState, engine: &EngineState) -> MyUniforms {
    let flash = if engine.audio.beat_pulse { 1.0_f32 } else { 0.0_f32 };
    // fade flash out in the shader using a time uniform
    MyUniforms { flash, .. }
}
```

### Beat phase

`beat_phase` is a sawtooth `[0, 1)` that resets to 0 on each detected beat. It's useful for smooth beat-locked animations.

```rust
let phase = engine.audio.beat_phase; // ramps 0→1 between beats
```

## Using audio in your effect

### Direct audio reactivity

Read FFT bands or volume directly in `build_uniforms()`:

```rust
fn build_uniforms(&self, s: &MyState, engine: &EngineState) -> MyUniforms {
    let bass = engine.audio.fft[0];
    MyUniforms {
        displacement: s.displacement_amount * bass,
        brightness:   0.5 + engine.audio.volume * 0.5,
        // ...
    }
}
```

### Via the routing matrix

For a more flexible setup — letting the user choose which FFT band drives which parameter at runtime — use the [Routing Matrix](routing.md). Routing contributions are already included when you call `engine.get_param()`.

## Tap tempo

The Audio tab has a tap-tempo button. The user can also tap `Shift+T` on the output window. Tap tempo overrides the audio BPM estimate but is itself overridden by Ableton Link or ProDJ Link if those sources are active.

For tempo-reactive effects, always use `engine.effective_bpm()` rather than `engine.audio.bpm` — it accounts for whichever source is active. See [Tempo Sync](tempo-sync.md).
