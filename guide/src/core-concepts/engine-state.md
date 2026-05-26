# EngineState

`EngineState` is the engine's live runtime state. It's passed to `build_uniforms()` every frame and to `prepare()`. Reading from it is how your effect reacts to audio, LFOs, tempo, and the current parameter values.

## Audio state

```rust
let volume     = engine.audio.volume;       // f32 — RMS loudness [0, 1]
let bass       = engine.audio.fft[0];       // f32 — lowest FFT band [0, 1]
let kick       = engine.audio.fft[1];       // f32 — next band up
let beat_pulse = engine.audio.beat_pulse;   // bool — true on beat onset
let bpm        = engine.audio.bpm;          // f32 — estimated BPM (audio only)
let beat_phase = engine.audio.beat_phase;   // f32 — [0, 1) position within the current beat
```

`fft` is an array of 8 bands covering the audible spectrum from bass to treble. Band 0 is the lowest frequencies; band 7 is the highest.

> When tempo sync is active, prefer `engine.effective_bpm()` and `engine.effective_beat_phase()` over `engine.audio.bpm`. See [Tempo Sync](../modulation/tempo-sync.md).

## Parameters

The `get_param` method returns a parameter's effective value — base slider value plus all active modulations (LFO, audio routing):

```rust
let intensity = engine.get_param("intensity").unwrap_or(0.5);
```

Returns `None` if no parameter with that id is registered. The returned value is already clamped to the parameter's declared `[min, max]` range.

## LFO state

You rarely need to read LFO state directly in `build_uniforms()` — declare parameters and `get_param()` includes LFO contributions automatically. But you can read raw LFO values if you need them:

```rust
let lfo_a_value = engine.lfo.banks[0].current_value; // f32 [-1, 1]
```

See [LFOs](../modulation/lfo.md).

## Tempo

```rust
// Always dispatches on the active sync source:
let bpm   = engine.effective_bpm();
let phase = engine.effective_beat_phase(); // [0, 1)

// Which source is active:
match engine.sync_source {
    SyncSource::Audio => { /* audio beat detection */ }
    SyncSource::Link  => { /* Ableton Link session  */ }
    SyncSource::ProDj => { /* ProDJ Link            */ }
}
```

`effective_bpm()` and `effective_beat_phase()` are the right calls for any tempo-reactive effect. They follow the active source automatically.

## MIDI Timecode (optional)

When the `mtc` feature is enabled:

```rust
if let Some(pos) = &engine.mtc.position {
    let seconds = pos.as_seconds_f64();
    let frame_rate = pos.frame_rate; // 24, 25, 29.97, 30
}
```

MTC is a position reference, not a BPM source — use it for timeline-locked visuals.

## Input / output state

```rust
// Current input dimensions
let (w, h) = (engine.input.width, engine.input.height);

// Whether an input source is active
let has_input = engine.input.active;
```

## Sending commands

`EngineState` uses command enums rather than direct mutation to change subsystem state. Commands are typically sent from a custom GUI tab, not from `build_uniforms()`. The engine processes them at the start of the next frame.

```rust
engine.input_commands.push(InputCommand::SetDevice(DeviceId(0)));
engine.output_commands.push(OutputCommand::EnableNdi(true));
engine.lfo_commands.push(LfoCommand::SetRate { bank: 0, hz: 1.0 });
```

You won't need these in simple effects, but they're how the built-in tabs work internally.
