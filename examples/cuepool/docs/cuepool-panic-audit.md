# CuePool panic audit

Audit date: 2026-08-12

Audited revision: `7aa9c4253809b70da4021c0dff3c9f32bd518f22`

## Scope and counting

CuePool's nested workspace sets `panic = "abort"` for release builds. Any panic in
the application therefore kills the whole CuePool process; a panic in a standalone
example kills that example process. Thread ownership below describes where the
panic originates, not a smaller blast radius.

The prescribed grep produced 156 matching source lines under `crates/`. Manual
`#[cfg(test)]`-module filtering removed 113 test-only lines. Two further matches
were not panic sites: the `.lock().unwrap()` text in the module comment at
`crates/cuepool-core/src/sync.rs:4`, and the `human_panic::setup_panic!` macro at
`crates/cuepool/src/main.rs:5417`. The remaining production surface is **44 panic
operations on 41 source lines**. The operation count is three higher because
`yuv_converter.rs:168`, `yuv_converter.rs:224`, and
`gen_lx_test.rs:92` each contain two audited operations. Standalone examples are
included because they are executable non-test code. There are no live `todo!` or
`unimplemented!` sites.

`assert!` and indexing are outside this task's requested primitive list. This is
therefore an audit of `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, and
`unimplemented!`, not a claim that no other Rust construct can panic.

Verdicts use these terms:

- **Reachable**: a concrete normal or malformed-input trigger is known.
- **Conditional**: the path is live, but the failing OS, device, or resource state
  was not reproduced.
- **Invariant-protected**: the operation runs, but the immediately preceding code
  establishes the value or variant it unwraps.
- **Dormant**: no production caller exists at this revision.

## Summary

Counts are panic operations, not matching lines.

| Owning thread / context | Critical | High | Medium | Low | Total |
|---|---:|---:|---:|---:|---:|
| Network RX (OSC/MSC/MIDI callbacks) | 0 | 0 | 0 | 0 | 0 |
| Audio callback | 2 | 0 | 0 | 0 | 2 |
| Render / video pipeline | 0 | 7 | 0 | 0 | 7 |
| Cue firing / show control | 0 | 13 | 0 | 0 | 13 |
| File load (`.qproj`, media) | 0 | 0 | 0 | 0 | 0 |
| Startup / device enumeration | 0 | 0 | 0 | 7 | 7 |
| GUI event handlers | 0 | 0 | 0 | 8 | 8 |
| Standalone developer examples | 0 | 0 | 0 | 7 | 7 |
| **Total** | **2** | **20** | **0** | **22** | **44** |

The absence of direct Network RX sites matters: production MSC, MIDI, and MTC
receive/parser code contains none of the audited primitives. The two OSC
operations are router-construction invariants executed on the main thread before
the RX thread starts, so they are Startup/Low rather than Network RX/Critical.

## Critical priority

These sites are invariant-protected today, but they execute in the audio callback.
If a backend ever violates the assumed buffer-format contract, abort occurs during
the show. They are the first defensive-hardening work items.

- `crates/cuepool-audio/src/engine.rs:325`: Thread: CPAL audio callback. Blast:
  whole CuePool process. Verdict: **invariant-protected**; the callback matches the
  captured format as `F32` before requesting an `f32` slice, but a CPAL/backend
  contract violation would abort instead of producing silence.
- `crates/cuepool-audio/src/engine.rs:579`: Thread: CPAL audio callback. Blast:
  whole CuePool process. Verdict: **invariant-protected**; the only production
  calls instantiate `T` as `i16` or `i32` from the matching format branch. Treat a
  mismatch as silence/error rather than retaining an abort in the callback.

## High priority

### Cue firing and show control

- `crates/cuepool-audio/src/engine.rs:429`: Thread: main/show-control while a
  sound cue is fired. Blast: whole CuePool process. Verdict: **conditional**; no
  failing production rate pair was established, but media/device-derived rates
  cross the `ResamplerProcessor` construction boundary and the error is converted
  to an abort. Malformed-media/device reachability remains unconfirmed.
- `crates/cuepool-audio/src/engine.rs:480`: Thread: intended main/show-control
  caller. Blast: whole CuePool process. Verdict: **dormant**; `build_cue_chain` has
  no production caller in the workspace, but it duplicates the same aborting
  resampler-construction policy and should be handled with the live site.
- `crates/cuepool/src/main.rs:971`: Thread: main/show-control during first output
  creation. Blast: whole CuePool process. Verdict: **conditional**; a GUI action or
  valid LAN GO for Text/Image/Video can initiate window creation, but failure also
  requires OS/window-resource state. Cannot determine reachability without running
  the GUI on the target rig.
- `crates/cuepool/src/main.rs:977`: Thread: main/show-control during first output
  creation. Blast: whole CuePool process. Verdict: **conditional**; surface creation
  is fallible after the window exists. Cannot determine reachability without the
  GUI/GPU combination used at the venue.
- `crates/cuepool/src/main.rs:982`: Thread: main/show-control during first output
  creation. Blast: whole CuePool process. Verdict: **conditional**; a surface with
  no default adapter configuration aborts. Cannot determine reachability without
  the GUI, GPU, monitor, and driver state.
- `crates/cuepool/src/main.rs:1115`: Thread: main/show-control while output render
  workers are created. Blast: whole CuePool process. Verdict: **conditional**;
  thread creation can fail under resource exhaustion. A LAN GO can initiate first
  output creation, but no exhaustion threshold was reproduced.
- `crates/cuepool/src/main.rs:1580`: Thread: main/show-control while firing a
  still-image PixelMap cue. Blast: whole CuePool process. Verdict:
  **invariant-protected**; `ensure_pixmap_texture` is called immediately before the
  unwrap and establishes `Some`.
- `crates/cuepool/src/main.rs:1595`: Thread: main/show-control while firing a
  video-backed PixelMap cue. Blast: whole CuePool process. Verdict:
  **conditional**; a valid OSC/MSC/MIDI GO can reach the spawn, and repeated GO on
  a retriggerable cue signals but does not join the prior decoder before spawning
  another. Resource-exhaustion reachability needs a GUI/project/flood run.
- `crates/cuepool/src/main.rs:1607`: Thread: main/show-control in
  `ensure_pixmap_texture`. Blast: whole CuePool process. Verdict:
  **invariant-protected**; the function assigns `Some` whenever the texture is
  absent or the size changes and performs no intervening clear.
- `crates/cuepool/src/main.rs:1623`: Thread: main/show-control during periodic
  PixelMap frame upload. Blast: whole CuePool process. Verdict:
  **invariant-protected**; the immediately preceding `ensure_pixmap_texture` call
  establishes the texture.
- `crates/cuepool/src/main.rs:1634`: Thread: main/show-control during a YUV
  PixelMap frame upload. Blast: whole CuePool process. Verdict:
  **invariant-protected**; `None` is replaced with a converter immediately before
  the unwrap.
- `crates/cuepool/src/main.rs:1636`: Thread: main/show-control during a YUV
  PixelMap frame upload. Blast: whole CuePool process. Verdict:
  **invariant-protected**; `ensure_pixmap_texture` established the texture earlier
  in the same branch and nothing clears it in between.
- `crates/cuepool/src/main.rs:2451`: Thread: main/show-control while firing or
  re-seeking a Video cue. Blast: whole CuePool process. Verdict: **conditional**;
  valid OSC/MSC/MIDI GO reaches this spawn, as can an MTC hard-sync correction.
  Repeated GO on a retriggerable video can transiently accumulate decoder threads;
  the actual resource-exhaustion threshold is unconfirmed without GUI/project/flood
  testing.

### Render and video pipeline

- `crates/cuepool-video/src/pixel_sampler.rs:207`: Thread: main/winit render tick
  at the configured DMX rate. Blast: whole CuePool process. Verdict:
  **invariant-protected**; the same branch inserts the segment when it is absent or
  stale, while the non-rebuild branch implies it already exists.
- `crates/cuepool-video/src/pixel_sampler.rs:268`: Thread: main/winit render tick
  after submitting pixel readbacks. Blast: whole CuePool process. Verdict:
  **invariant-protected**; every ID in `kicked` came from a successful mutable lookup
  earlier in `sample`, and the map is not mutated between the two loops.
- `crates/cuepool-video/src/yuv_converter.rs:168` (`unwrap`): Thread:
  video-consume worker for normal video, or main/winit for PixelMap video. Blast:
  whole CuePool process. Verdict: **invariant-protected**; `None` or a non-Planar
  binding makes `need_realloc` true and installs a Planar binding first.
- `crates/cuepool-video/src/yuv_converter.rs:168` (`unreachable!`): Thread:
  video-consume worker or main/winit PixelMap path. Blast: whole CuePool process.
  Verdict: **invariant-protected** by the same variant-replacement branch; no state
  mutation intervenes between assignment and destructuring.
- `crates/cuepool-video/src/yuv_converter.rs:224` (`unwrap`): Thread:
  video-consume worker for normal video, or main/winit for PixelMap video. Blast:
  whole CuePool process. Verdict: **invariant-protected**; `None` or a non-NV12
  binding causes immediate installation of an NV12 binding.
- `crates/cuepool-video/src/yuv_converter.rs:224` (`unreachable!`): Thread:
  video-consume worker or main/winit PixelMap path. Blast: whole CuePool process.
  Verdict: **invariant-protected** by the same local variant invariant.
- `crates/cuepool/src/main.rs:3650`: Thread: main/winit control-window render.
  Blast: whole CuePool process. Verdict: **conditional**; it is reached only after
  wgpu reports the control surface lost, then aborts if recreation fails. Cannot
  determine reachability without running the GUI through device/surface loss.

## Medium priority

There are no audited panic primitives owned by the file-load path. `.qproj`
deserialization and migration return errors rather than unwrapping, and the
showfile fuzz campaigns described below found zero panics. Media loading can lead
to High cue-firing sites after a project is accepted, but no site is owned directly
by file load.

## Low priority

### Startup and device enumeration

- `crates/cuepool-gui/src/logging.rs:38`: Thread: main startup. Blast: whole
  CuePool process. Verdict: **conditional**; `log::set_boxed_logger` fails if a
  logger was already installed. The binary calls this once and no earlier logger
  installation was found.
- `crates/cuepool-protocols/src/osc/mod.rs:200`: Thread: main startup while the
  internal OSC routes are subscribed, before RX starts. Blast: whole CuePool
  process. Verdict: **invariant-protected**; the immediately preceding branch
  installs the wildcard node when absent.
- `crates/cuepool-protocols/src/osc/mod.rs:212`: Thread: main startup while the
  internal OSC routes are subscribed, before RX starts. Blast: whole CuePool
  process. Verdict: **invariant-protected**; the same key is inserted when missing
  immediately before `get_mut`, under exclusive access to the router.
- `crates/cuepool/src/main.rs:714`: Thread: main startup. Blast: whole CuePool
  process. Verdict: **conditional**; creating the long-lived video-consume thread
  can fail under OS resource exhaustion, before the show UI is ready.
- `crates/cuepool/src/main.rs:801`: Thread: main startup/resume. Blast: whole
  CuePool process. Verdict: **conditional**; control-window creation is fallible
  and requires the GUI/window system to classify further.
- `crates/cuepool/src/main.rs:807`: Thread: main startup/resume. Blast: whole
  CuePool process. Verdict: **conditional**; control-surface creation depends on
  the GUI/GPU backend and was not exercised by the parser fuzz tasks.
- `crates/cuepool/src/main.rs:812`: Thread: main startup/resume. Blast: whole
  CuePool process. Verdict: **conditional**; absence of a compatible default
  surface configuration aborts before the show and should be reported as setup
  failure instead.

### GUI event handlers

- `crates/cuepool-core/src/showfile/mod.rs:83`: Thread: main GUI event while
  adding or duplicating a cue. Blast: whole CuePool process. Verdict:
  **invariant-protected**; parses the source literal `"0.1"`.
- `crates/cuepool-core/src/showfile/mod.rs:84`: Thread: main GUI event while
  adding or duplicating a cue. Blast: whole CuePool process. Verdict:
  **invariant-protected**; parses the source literal `"0.01"`.
- `crates/cuepool-core/src/showfile/mod.rs:85`: Thread: main GUI event while
  adding or duplicating a cue. Blast: whole CuePool process. Verdict:
  **invariant-protected**; parses the source literal `"0.001"`.
- `crates/cuepool-core/src/showfile/mod.rs:86`: Thread: main GUI event while
  adding or duplicating a cue. Blast: whole CuePool process. Verdict:
  **invariant-protected**; parses the source literal `"0.0001"`.
- `crates/cuepool-core/src/showfile/mod.rs:87`: Thread: main GUI event while
  adding or duplicating a cue. Blast: whole CuePool process. Verdict:
  **invariant-protected**; parses the source literal `"0.00001"`.
- `crates/cuepool-core/src/showfile/mod.rs:88`: Thread: main GUI event while
  adding or duplicating a cue. Blast: whole CuePool process. Verdict:
  **invariant-protected**; parses the source literal `"0.000001"`.
- `crates/cuepool-gui/src/app/mod.rs:84`: Thread: main GUI event while merging
  undo snapshots. Blast: whole CuePool process. Verdict: **invariant-protected**;
  `last_mut` follows a successful `last` lookup and the vector is not mutated
  between them.
- `crates/cuepool-gui/src/lighting_panel.rs:496`: Thread: main GUI event while
  creating/importing a user fixture profile. Blast: whole CuePool process.
  Verdict: **invariant-protected in practical operation**; `find` scans the
  unbounded `(1..)` ID sequence, so returning `None` would require exhausting the
  integer sequence/address space rather than operator-controlled profile content.

### Standalone developer examples

These binaries are not called by the CuePool application, so their blast radius is
the example process rather than the unattended show process.

- `crates/cuepool-core/examples/gen_lx_test.rs:91`: Thread: standalone example
  main. Blast: whole `gen_lx_test` process. Verdict: **reachable** when the output
  path argument is omitted; this is deliberate CLI usage enforcement.
- `crates/cuepool-core/examples/gen_lx_test.rs:92` (serialization `unwrap`):
  Thread: standalone example main. Blast: whole `gen_lx_test` process. Verdict:
  **invariant-protected** for the fixed, serializable `ShowFile` constructed by the
  example; no failing value was found.
- `crates/cuepool-core/examples/gen_lx_test.rs:92` (write `unwrap`): Thread:
  standalone example main. Blast: whole `gen_lx_test` process. Verdict:
  **reachable** with an unwritable, missing-parent, or otherwise invalid output
  path.
- `crates/cuepool-video/examples/decode_check.rs:16`: Thread: standalone example
  main. Blast: whole `decode_check` process. Verdict: **reachable** when the media
  path argument is omitted; deliberate CLI usage enforcement.
- `crates/cuepool-video/examples/decode_check.rs:17`: Thread: standalone example
  main. Blast: whole `decode_check` process. Verdict: **reachable** when the
  optional frame-count argument is not a `u32`.
- `crates/cuepool-video/examples/decode_smoke.rs:12`: Thread: standalone example
  main. Blast: whole `decode_smoke` process. Verdict: **reachable** when the media
  path argument is omitted; deliberate CLI usage enforcement.
- `crates/cuepool/examples/play_file.rs:18`: Thread: standalone example main.
  Blast: whole `play_file` process. Verdict: **reachable** when the audio path
  argument is omitted; deliberate CLI usage enforcement.

## Confirmed reachable panics from Tasks 5-7

**None.** The showfile, MSC, and OSC fuzz tasks all completed with zero panics, so
there is no reproducing crash seed to record.

The deterministic campaigns were:

| Surface | Coverage | Campaign seed(s) | Result |
|---|---|---|---|
| `.qproj` showfile | 20,000 arbitrary byte inputs; 10,000 structure-aware corruptions including migration | `0xDEC0DE`, `0xC0FFEE11` | Zero panics |
| MSC UDP parser | 50,000 arbitrary packets; 2,000 truncation campaigns; 40,000 correctly framed hostile payloads plus every prefix | `0x4D5343`, `0xBADBEE`, `0x6D736364` | Zero panics |
| OSC router | 20,000 randomized addresses/arguments across 11 patterns with a watchdog | `0x05C` | Zero panics or hangs |

This makes CuePool's MSC parser and post-decode OSC routing surfaces fuzz-clean for
the bounded campaigns that ran; it is not a proof over every input. OSC fuzzing did
not fuzz `rosc::decoder::decode_udp` itself, whose errors are matched and logged.
There was no MIDI fuzz task, although the production MIDI/MTC receive code contains
no audited panic primitive.

The two remaining OSC unwraps at `osc/mod.rs:200` and `osc/mod.rs:212` are setup
sites, not network-RX parser sites. The MSC and MTC unwraps found by the raw grep
were all inside `#[cfg(test)]` modules.

## Deferred clippy findings (not panic sites)

- `crates/cuepool-audio/src/mixer.rs:319`: Thread: audio callback render path.
  Reachability: **reached for every rendered frame while a fade is active**. The
  current `saturating_sub(1)` cannot underflow or panic and preserves zero after the
  fade completes. This is Critical-thread code but not a panic work item at the
  audited revision.
- `crates/cuepool/src/main.rs:3720`: Thread: main/winit control render.
  Reachability: **reached on GUI frames while an audio-less video is active**. At
  this revision the cited line is `let position_secs = video_pos_secs`, not an
  identical if-block. The debt-cleanup stack already consolidated the identical
  F11 and Ctrl/Cmd+F fullscreen branches. There is no live clippy or panic finding
  at the task's cited line; the citation was stale by the harness+fuzz tip.

## Priority-ordered follow-up worklist

1. Remove the two callback aborts at `engine.rs:325` and `engine.rs:579`; a backend
   contract failure should produce silence and diagnostics, not kill the show.
2. Make decoder/output thread creation failures non-aborting, starting with the
   remotely triggerable/retriggerable paths at `main.rs:1595` and `main.rs:2451`,
   then output-render creation at `main.rs:1115`.
3. Propagate or surface window/surface/configuration failures at `main.rs:971`,
   `main.rs:977`, `main.rs:982`, and `main.rs:3650`; verify them on the target GUI,
   GPU, and monitor topology because static analysis cannot establish failure
   reachability.
4. Remove the duplicated resampler-construction abort policy at `engine.rs:429`
   and dormant `engine.rs:480`, returning a cue error through the existing
   show-control path.
5. Collapse invariant-protected unwraps only when those files are otherwise being
   changed. They have no known failing input and rank below the fallible OS/resource
   sites; avoid broad churn solely to make the grep count zero.
6. Leave standalone-example usage panics as Low unless those tools become operator
   workflows; the reachable filesystem write error at `gen_lx_test.rs:92` is the
   only one worth normal CLI error reporting now.

## Addendum: changes on `main` after the audited revision (2026-08-13)

Two changes landed between the audited revision and this document merging:

- PR #98 removed the two Critical audio-callback aborts (`engine.rs:325` and
  `engine.rs:579` above); a sample-format mismatch now silences the callback
  instead of aborting. Worklist item 1 is done. `engine.rs` line numbers in this
  document predate that change — the resampler-construction sites from worklist
  item 4 now sit at `engine.rs:443` and `engine.rs:494`.
- PR #99 synced the nested `Cargo.lock` with the cpal 0.18 manifests.

Worklist items 2–6 are unaffected and remain open.
