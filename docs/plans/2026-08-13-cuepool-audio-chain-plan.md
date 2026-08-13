# CuePool per-cue DSP chain consolidation plan

Issue: [#126](https://github.com/BlueJayLouche/rustjay-engine/issues/126)

## Goal

One library entry point in `cuepool-audio` builds and plays the per-cue chain;
`main.rs::play_audio` calls it. **Zero audible behavior change.**

## Current state (verified at commit 874b364)

- `AudioEngine::build_cue_chain` (`cuepool-audio/src/engine.rs:486`) is dead
  code: nothing calls it, its body only resamples and upmixes, and its doc
  comment describes a processor order that does not ship.
- `main.rs::play_audio` (1922–2034) hand-wires the real chain:
  `FileDecoder` → `LoopProcessor::new` + `set_loop(start_frame, end_frame,
  loop_mode, loop_count)` + optional `with_loop_counter(Arc<AtomicU32>)`
  → optional `EqProcessor` (with `eq_settings.enabled` forced `true` — covers
  show-files saved before that fix) → optional fade-in `FadeProcessor`
  (`fade_in > 0.0`: `start_fade(1.0, fade_in * source_rate, fade_type)`)
  → `AudioEngine::play`, which appends resampler → `MonoToStereo` →
  `BufferedSource` → `MixerInput`.
- So the real order is Loop → EQ → FadeIn **at source rate**, then resample and
  upmix. Preserve exactly this order: fade frame counts and EQ biquads are
  computed at the source rate, before the resampler.
- The loop counter is created iff `loop_mode` is `Looped`/`LoopedInfinite` and
  must be surfaced to the caller — the main thread reads it to sync video
  restarts and progress-bar resets.
- Fade-out is **not** part of the chain: it goes through
  `MixerInput::start_fade` (main.rs 2088, 2319, 2393, 2412). Leave it alone.

## Design

- New params struct in `cuepool-audio`, e.g. `CueChainParams { start_frame:
  u64, end_frame: u64, loop_mode: LoopMode, loop_count: u32, eq:
  Option<EQSettings>, fade_in_secs: f32, fade_type: FadeType }` (match the
  types main.rs already passes; check the real signatures before writing).
- Replace `build_cue_chain` with something like
  `pub fn play_cue(&self, source: Box<dyn SampleProvider>, params:
  CueChainParams) -> Result<CuePlayback, AudioError>` where
  `CuePlayback { input: Arc<MixerInput>, loop_counter:
  Option<Arc<AtomicU32>> }`. Internally: build Loop → EQ → FadeIn, then
  delegate to the existing `play` for resample/upmix/buffer/mixer. The
  force-enable EQ rule moves into the library with a comment saying why.
- `AudioEngine::play` stays public and unchanged (check its other callers
  before touching anything about it).
- Delete the dead `build_cue_chain`; the new entry point's doc comment states
  the real chain order.
- `main.rs::play_audio` shrinks to: resolve path, open `FileDecoder`, compute
  start/end frames and `out_scale` (unchanged), call `play_cue`, then apply
  `set_volume` / `set_pan` / `set_routing` / preload `set_active(false)` to the
  returned input.

## Tests (cuepool-harness: null sink + virtual clock)

- Looped cue: the loop counter increments at a loop boundary.
- Fade-in: output amplitude ramps from silence to full over the fade span
  (approximate assertions are fine; curve exactness is not the point).
- EQ passed as `Some(settings)` with `enabled = false` still processes
  (the force-enable rule).
- Chain-order regression: a mono source at a non-device rate through `play_cue`
  comes out stereo at device rate (resample and upmix still applied after the
  front chain).

## Gates

From `examples/cuepool/` (nested workspace — never the repo root), same as CI:

- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`

Commit this plan doc first, then small logical commits
(`cuepool: <what changed>`).

## Out of scope

- #80 (ASIO 24-bit device enumeration) — Windows-only, needs hardware.
- Internals of `FadeProcessor` / `EqProcessor` / `LoopProcessor`.
- All fade-out paths.
