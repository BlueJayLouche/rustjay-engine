# Engine-state frame lock design

Issue #19 asks the per-frame update path to stop acquiring `EngineState` once
per subsystem. A literal guard around all of `about_to_wait` is unsafe: the
modulation engine has a documented reverse lock path fixed by `9b0be46b`, audio
reconnection opens hardware, and the renderer acquires `EngineState` itself.

## Approved design

Use a safe two-phase frame lock:

1. Keep command dispatch separate, then perform rare audio reconnection before
   entering the ordinary update guard.
2. Acquire `EngineState` once for discovery polling, input, audio analysis,
   Link/ProDJ, and the LFO input snapshot. Pass `&mut EngineState` into these
   functions instead of letting them lock `App::shared_state`.
3. Drop the guard before locking and ticking the modulation engine. This is the
   authoritative `9b0be46b` deadlock boundary and must remain explicit in code.
   Hardware-backed MIDI/MTC availability work also runs while the state guard is
   absent.
4. Reacquire `EngineState` to commit LFO offsets and run the remaining state
   mutations for MIDI, OSC, web publication, and device-discovery completion.
   Drop it before settings I/O and rendering; those paths either block or own
   their own state-lock lifecycle.

Use the whole `&mut EngineState` rather than a derived borrow struct. The update
functions span input, audio, sync, control, and web fields, so a narrow facade
would duplicate `EngineState`'s shape without enforcing a useful invariant.
The function signatures themselves remove the repeated lock sites with the
smallest diff.

## Lock safety

Input polling drains channels without waiting. Audio analysis reads atomics and
copies cached buffers. Link and ProDJ perform their existing per-frame snapshot
updates. MIDI and OSC take only their own short-lived state mutexes; device
enumeration and disconnect/reconnect operations remain outside the frame guard.
The web modulation snapshot is deferred until after the frame guard drops so it
never nests the modulation mutex under `EngineState`.

Rendering remains outside because `OutputEngine::render` locks `EngineState`
internally. Settings persistence remains outside because it performs filesystem
I/O. These are intentionally not part of the ordinary update guard.

## Regression check

Add a debug-only frame-lock re-entrancy marker and a unit test proving a nested
frame acquisition panics before it can deadlock. Update functions receive state
borrows and contain no direct frame-lock acquisition.

## Alternatives rejected

- A literal single guard for all of `about_to_wait` would violate the modulation
  lock order, deadlock in rendering, and hold the audio-contended mutex over I/O.
- A derived frame-borrow struct would touch more fields and files while providing
  no stronger safety than direct Rust borrows for the current update pipeline.
