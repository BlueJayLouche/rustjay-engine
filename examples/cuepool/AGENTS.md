# CuePool development

CuePool is a standalone Cargo workspace. Run its commands from this directory, not the repository root.

## Show behavior

- `crates/cuepool/src/engine.rs` is the source of truth for cue sequencing and lifecycle behavior. Put Go, Stop, pause, seek, delay, WithLast, AfterLast, loop, and EOF changes in `ShowEngine`.
- Keep `crates/cuepool/src/main.rs` as the winit adapter for windows, GPU presentation, device audio configuration, protocols, and lighting I/O. Do not add a second scheduler there.
- Keep engine time explicit. Commands, events, and ticks receive a monotonic `Duration`; `ShowEngine` must not read `Instant` or wall-clock time directly.
- Apply emitted `EngineAction`s in order before the next tick. Report asynchronous completion through `EngineEvent`, including the video instance and epoch so stale EOF events cannot finish a replacement stream.
- Add or change the shared engine behavior instead of writing a test-only cue interpreter.

## Headless show tests

Use `cuepool_harness::HeadlessShowRunner` for full-show behavior that does not require a window, GPU, audio device, or external I/O:

1. Open a real format-v9 `.qproj` with `HeadlessShowRunner::open`.
2. Select a cue explicitly, then call `go`, `pause`, `resume`, `seek`, or `stop`.
3. Advance playback with `advance_blocks`. Each block renders through `NullSink`, advances `VirtualClock`, consumes due FFmpeg frames, and ticks `ShowEngine`.
4. Assert stable state with `snapshot`; use `take_trace` for ordered cue, frame, EOF, and side-effect events. `take_trace` drains the accumulated trace.

Sound and video use the production decoder paths. Network, text, image, PixelMap, lighting, and DMX actions are recorded without sending external I/O.

Generate test projects and media in a unique standard-library temporary directory, reference media with relative paths, and remove the directory after the test. Do not commit media fixtures or require the FFmpeg CLI. See `crates/cuepool-harness/tests/headless_show.rs` and its `support` module for the established pattern.

## Verification

Run the focused checks first:

```sh
cargo test -p cuepool-harness --tests
cargo test -p cuepool
```

Before opening a PR, run:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Headless tests cover show logic and decoder timing. They do not prove presentation cadence, vsync behavior, audio-device routing, protocols, lighting hardware, or projector output. Changes at those boundaries still need an attended binary or rig smoke test.
