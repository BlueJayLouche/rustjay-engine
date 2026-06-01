# Perf pass: video-input upload path — Phase 0 findings

Branch: `perf/gpu-input-path`

## Summary / decision

- **Phase 1 (skip uploading unused slots): FEASIBLE — implemented.**
  Requires one additive, defaulted trait method `EffectPlugin::input_count() -> u32`
  (defaults to `1`), overridden by `waaaves` (the only example that samples a second
  input) to return `2`. All other examples are unchanged.
- **Phase 2 (bypass the CPU memmove for GPU-capable captures): NOT FEASIBLE — skipped.**
  Of the byte-path backends, none can hand wgpu a GPU/IOSurface/CVPixelBuffer-backed
  texture without a capture-library rewrite. The only backend that already delivers a
  GPU texture (macOS Syphon) is *already* on the zero-copy `set_external_texture` path,
  so there is nothing to convert. See item 2 below.

## 1. Plugin input usage — does `EffectPlugin` expose slot count?

**No.** `EffectPlugin` (`crates/rustjay-core/src/plugin.rs:107`) has no method describing
how many input slots an effect samples. The *only* signal that an effect uses a second
input today is implicit: its `render()` override reads `engine.second_input_view`.

Grep across `examples/*/src`:
- `waaaves/src/main.rs:380` — `engine.second_input_view.as_deref()` → **uses 2 inputs**.
- `delta`, `delta-egui`, `flux`, `sputnik`, `template`, `isf-example`, `webapp` — none
  reference `second_input_view` → **use 1 input**.

`flux` and `delta` `render()` overrides consume only the first slot
(`input_view` / `input_texture`), confirmed at `examples/flux/src/main.rs:404-420` and
`examples/delta/src/main.rs:432-460`.

**Smallest additive change:** a defaulted trait method
```rust
fn input_count(&self) -> u32 { 1 }
```
`waaaves` overrides it to return `2`. Default `1` keeps every other example compiling and
behaving identically. The `App` already owns the active plugin (`App::plugin: Option<P>`,
`crates/rustjay-engine/src/app/mod.rs:173`), so `update.rs` can read it directly.

## 2. Capture backends — CPU bytes vs GPU texture?

| Backend | OS | What it delivers | Source |
|---|---|---|---|
| Webcam / v4l | all | CPU BGRA `Vec<u8>` (`WebcamFrame.data`) | `crates/rustjay-io/src/input/webcam.rs:12-17`, YUY2→BGRA convert at `:83-112` |
| NDI | all | CPU BGRA `Vec<u8>` (`NdiFrame.data`), stride-stripped | `crates/rustjay-io/src/input/ndi.rs:22-23,128,152,228-230` |
| Syphon | macOS | **GPU `wgpu::Texture` (IOSurface-backed)** | `crates/rustjay-io/src/input/syphon_input.rs`; consumed via `syphon_output_texture()` `input/mod.rs` |
| Spout | Windows | CPU pixels `&[u8]` via `spout_pixels()` | `crates/rustjay-io/src/input/spout_input.rs`, `input/mod.rs:spout_pixels` |

Conclusion: webcam, NDI, and Spout produce **CPU byte buffers only** — there is no
GPU/IOSurface/CVPixelBuffer handle to route through `set_external_texture`. The one
GPU-capable backend, Syphon, is already handled on the zero-copy external-texture path
(`update.rs:35-55`), so Phase 2 has no remaining target. Converting webcam/NDI to a
GPU-backed delivery would be a capture-library rewrite, which the task explicitly excludes.
**Phase 2 is skipped.**

## 3. Correctness dependency — `raw_input` consumers

`set_external_texture` (`texture.rs:266-285`) deliberately also blits into `self.texture`
via `update_from_texture`, so plugins that read the *owned* input texture still see a
non-`None` value. Consumers of the owned texture (`render()`'s `input_texture` arg):
- `delta` — `FrameHistory` ring buffer, `examples/delta/src/main.rs:457-459`
  (`history.push_frame(src, encoder)`).
- `flux` — `examples/flux/src/main.rs:417` (`input_texture` required).

Phase 1 only gates the **second** slot's *texture upload*; it never touches slot 1 or the
owned-texture semantics, so `delta`/`flux` `raw_input` is unaffected. Phase 2 (which would
have needed to preserve these semantics) is not implemented.

## Phase 1 implementation notes

- `update_input()` (`update.rs`) now uploads slot 2 only when
  `plugin.input_count() >= 2`. Device housekeeping (`manager.update()`, NDI source-lost
  detection, and draining `take_frame()`) still runs every frame; only the GPU
  `write_texture` / `set_external_texture` upload is skipped, per the task's
  backpressure guidance.
- No frame-byte hash/dirty check was added (the upload remains gated on
  `take_frame() == Some`).

## Measurement

The original `sample` profile (cited in the task brief: live webcam slot 1 + NDI slot 2,
single-input effect) showed ~873 `write_texture` samples split across both slots
(~568 on slot 2, dominated by `_platform_memmove`).

**I did not capture an after-profile.** This environment has no display / webcam / NDI
source, so I cannot launch the macOS GUI app or exercise the input path live, and I will
not fabricate sample counts. Correctness of the skip is instead guaranteed by the code
path: for any effect with `input_count() == 1` (the default — every example except
`waaaves`), `update_input()` computes `second_needed = false` and calls
`update_input_slot(true, /* upload_texture */ false)`. Every `engine.*_input_texture
.update(...)` / `set_external_texture(...)` call in that function is now wrapped in
`if upload_texture { … }`, so the slot-2 upload — and therefore its `write_texture` /
`_platform_memmove` — is statically unreachable for single-input effects. Slot 2 still
runs `manager.update()` and drains `take_frame()`, so no device backpressure is introduced.

For a true before/after `_platform_memmove` count on the Pi/llvmpipe target, run a
`samply`/`sample` capture with `sputnik` (or any default `input_count()==1` effect) and a
second input configured, before and after this branch. **Left as follow-up** — I could not
measure it here.
