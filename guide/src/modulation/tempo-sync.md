# Tempo Sync

rustjay-engine can lock to three external tempo sources. Enabling them is a feature-flag choice; selecting between them at runtime is a user choice from the Audio tab.

## Sources

### Audio beat detection (always available)

The default. The engine analyses the audio input to estimate BPM and track beat phase. Also responds to tap tempo (`Shift+T` on the output window, or the tap button in the Audio tab).

No features needed. Works offline. Quality depends on the audio signal.

### Ableton Link (`link` feature)

Joins the local Link session — synchronises BPM and beat phase with Live, Serato, Traktor, or any other Link-enabled app on the same network.

```toml
[dependencies]
rustjay-engine = { git = "...", features = ["link"] }
```

**Requires CMake** (`brew install cmake` / `apt install cmake`). Linking against Ableton Link makes the resulting binary **GPL-2.0+**.

When Link peers are present, the engine joins the session automatically once the user activates the Link source. The Sync tab shows the current peer count, beat phase bar, and quantum (loop length) slider.

### ProDJ Link (`prodj` feature)

Receives BPM, beat phase, and track metadata from Pioneer CDJ/XDJ/DJM gear on the same LAN.

```toml
[dependencies]
rustjay-engine = { git = "...", features = ["prodj"] }
```

No extra system dependencies. Binds UDP ports 50000 and 50002 — get operator approval before using on a production DJ network. The Sync tab shows the connected decks and which is the master.

### MIDI Timecode (`mtc` feature)

Decodes SMPTE timecode from any connected MIDI device. MTC is a position reference, not a BPM source — use it for timeline-locked visuals rather than beat-reactive effects.

```toml
[dependencies]
rustjay-engine = { git = "...", features = ["mtc"] }
```

No extra system dependencies. Listens on all MIDI ports simultaneously.

## Using multiple features

Any combination works:

```toml
rustjay-engine = { git = "...", features = ["link", "prodj", "mtc"] }
```

The user selects the active source at runtime from the Audio tab.

## Writing sync-aware plugins

Use `effective_bpm()` and `effective_beat_phase()` instead of the `audio.*` fields:

```rust
fn build_uniforms(&self, s: &MyState, engine: &EngineState) -> MyUniforms {
    MyUniforms {
        bpm:        engine.effective_bpm(),
        beat_phase: engine.effective_beat_phase(),
        // ...
    }
}
```

These dispatch on `engine.sync_source` automatically — your plugin works correctly regardless of which source the user has selected.

### Beat-locked animation

A common pattern: animate a value based on beat phase so it resets on each beat.

```rust
let phase = engine.effective_beat_phase(); // [0, 1)
let flash = (1.0 - phase).powf(4.0);      // bright on beat, decays quickly
```

In the shader:

```wgsl
let brightness = u.flash * 2.0 + base_brightness;
```

### BPM-locked oscillation

Use BPM to drive a shader oscillation without LFOs:

```rust
// Pass BPM and time to the shader and oscillate at a beat subdivision
let bpm   = engine.effective_bpm();
let phase = engine.effective_beat_phase();
MyUniforms { bpm, beat_phase: phase, .. }
```

```wgsl
// In shader: oscillate at 2× the beat rate
let osc = sin(u.beat_phase * 2.0 * 3.14159 * 2.0);
```

## Reading MIDI Timecode position

```rust
fn build_uniforms(&self, s: &MyState, engine: &EngineState) -> MyUniforms {
    let time_seconds = engine.mtc.position
        .as_ref()
        .map(|p| p.as_seconds_f64() as f32)
        .unwrap_or(0.0);
    MyUniforms { time_seconds, .. }
}
```
