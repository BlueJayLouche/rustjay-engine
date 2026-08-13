# CuePool convert-on-sample design

Issue: [#139](https://github.com/BlueJayLouche/rustjay-engine/issues/139)

## Decision

Do not begin with the full convert-on-sample change. First add a field-only
timing split inside the pinned wgpu fork, then fold each frame's existing YUV
conversion command buffer into the next output thread's already-required queue
submit. That is the smallest credible fix: it removes the extra shared-fence
lock acquisition while preserving the RGBA canvas and every current consumer.

Run the ASHOF-PC12 gate after that change. If it restores 48–50 uploads/s with
near-zero drops, stop; convert-on-sample is YAGNI. If it does not, or the split
shows that canvas conversion/bandwidth rather than the extra submission is the
remaining ceiling, implement the Track B design below for CPU-uploaded YUV
planes. Keep D3D11 zero-copy as convert-to-canvas, but put its acquire, convert,
release, and output work in one existing render-thread submit.

This recommendation assumes the 14.7–14.8 ms is mainly a per-call submit
serialization cost rather than GPU execution saturation. If that assumption is
false, batching removes the consume-thread wait but may not restore presentation
headroom; the rig gate then promotes convert-on-sample from contingency to the
next implementation stage.

## Scope and evidence boundary

The field baseline is ASHOF-PC12, RTX 4000, driver 596.49, Vulkan, three
1920x1200 outputs at 50 Hz, and 5120x1200 50p HEVC: about 29 uploads/s, 21
drops/s, zero starvation, 47–50 presents/s on each output, 14 event-loop
iterations/s, 2.93 ms upload time, and 14.7–14.8 ms conversion-submit time on
both D3D11VA readback and direct D3D11 NV12. Those observations come from issue
#139; source inspection can explain a credible mechanism but cannot attribute
the measured 15 ms to one lock or driver call.

CuePool pins `wgpu`, `wgpu-core`, `wgpu-hal`, and `wgpu-types` 29.0.4 to fork
revision `a0b9d5718c2ab2edfce2f156792b0f4359f25460`. The revision is fixed by
`examples/cuepool/Cargo.toml::patch.crates-io` and confirmed by
`examples/cuepool/Cargo.lock`. All wgpu claims below refer to that revision, not
current upstream.

Current CuePool behaviour is cited as `file::symbol`, deliberately without line
numbers. The branch moved these modules recently; symbols are the stable review
anchor. Historical comments introduced by `daa67a8` and `c90dd2d` remain useful
field evidence, but statements about closed-source NVIDIA WSI behaviour are not
promoted to source-proven facts.

Non-goals are a new dependency, a new queue, synchronized scanout across
ungenlocked projectors, a rewrite of the independent PixelMap cue pipeline, or
removal of the RGBA canvas used by still and text content.

## Current frame path

The decode thread feeds a bounded three-frame channel. The consume thread wakes
from output 0's present tick, selects the newest due frame, uploads CPU planes,
records a YUV-to-RGB pass into the RGBA canvas, submits it, and publishes shared
canvas/overlay views. Each output thread independently loops acquire, encode,
submit, present. Sources:
`examples/cuepool/crates/cuepool/src/main.rs::{VIDEO_QUEUE_CAP,App::new}` and
`examples/cuepool/crates/cuepool/src/video_pipeline::{video_decode_thread,video_consume_thread,output_render_thread,OutputFrameState}`.

```text
decode thread -> bounded frames -> consume thread
                                  | upload planes
                                  | submit YUV -> RGBA canvas   <-- 15 ms call
                                  v
                         OutputFrameState (canvas + overlay)
                           /              |              \
                    output 0          output 1          output 2
                 acquire/submit     acquire/submit    acquire/submit
                    /present           /present          /present

RGBA canvas ----> PixelSampler ----> LightingEngine ----> DMX outputs
text overlay -------------------------------------> output renderers
```

The output threads already achieve the required presentation rate. The delivery
ceiling occurs before publication: newest-due selection discards the source
frames that became late while the consume thread was inside the standalone
conversion submit. Sources:
`examples/cuepool/crates/cuepool/src/video_pipeline::{video_consume_thread,TimingWindow}`
and `examples/cuepool/crates/cuepool-gui/src/app::{Diagnostics::sections,VideoTimings}`.

## Track A: understand and remove the stall at the smallest rung

### What `Queue::submit` can contend on

The public `wgpu::Queue::submit` first takes each command buffer's deferred
actions mutex, calls the backend, and runs the deferred actions after the backend
returns. Those command buffers are owned by their caller, so that mutex exists
but is not a credible cross-thread 15 ms lock in the current topology. Source:
pinned `wgpu/src/api/queue.rs::Queue::submit`.

The important shared lock is `Device::fence`:

- `wgpu_core::device::queue::Queue::submit` takes
  `device.snatchable_lock.read()`, then `device.fence.write()`, then
  `device.command_indices.write()`. The fence write guard remains held across
  command validation, tracker transitions, pending-write preparation, the HAL
  submit, and post-submit maintenance. Source: pinned
  `wgpu-core/src/device/queue.rs::Queue::submit`.
- `wgpu_core::present::Surface::get_current_texture` takes
  `device.fence.read()` and holds it across the HAL surface acquisition. On
  Vulkan that acquisition can wait for the previous submission that used the
  acquire semaphore, call `vkAcquireNextImageKHR`, and then call
  `vkWaitForFences`. The pinned Vulkan source explicitly says the final wait is
  important on Windows/DXGI for frame pacing. Three FIFO output threads can
  therefore hold concurrent fence read guards while blocked for their respective
  images, preventing the consume thread from acquiring the write guard. Sources:
  pinned `wgpu-core/src/present.rs::Surface::get_current_texture` and
  `wgpu-hal/src/vulkan/swapchain/native.rs::NativeSwapchain::acquire`.
- `wgpu_core::present::Surface::present` takes the same
  `device.fence.write()` across the HAL present; Vulkan holds that surface's
  swapchain write guard and calls `vkQueuePresentKHR`. A submit can therefore
  wait behind a present call as well. Sources: pinned
  `wgpu-core/src/present.rs::Surface::present`,
  `wgpu-hal/src/vulkan/mod.rs::Queue::present`, and
  `wgpu-hal/src/vulkan/swapchain/native.rs::NativeSwapchain::present`.

Once it has the fence write guard, core submit also takes the device tracker
mutex while resolving transitions, the queue's `pending_writes` mutex while
preparing queued `write_texture`/`write_buffer` work, and the queue lifetime
tracker during cleanup. `pending_writes` stays held across the HAL submit, so a
render-thread submit can also delay the consume thread's next plane upload; the
field's 2.93 ms upload value may include some of that contention. Source: pinned
`wgpu-core/src/device/queue.rs::{Queue::submit,Queue::maintain}`.

The Vulkan HAL then takes per-surface acquire/present semaphore guards for any
surface textures in that submission, briefly updates its signal-semaphore and
relay-semaphore mutexes, maintains the wgpu fence, and calls `vkQueueSubmit` on
the one Vulkan queue. The relay semaphores put every submission into a strict
queue order. Sources: pinned `wgpu-hal/src/vulkan/mod.rs::{Queue,Queue::submit}`
and `wgpu-hal/src/vulkan/swapchain/native.rs::{SwapchainAcquireSemaphore,SwapchainPresentSemaphores}`.

Post-submit `Device::maintain(Poll)` does not deliberately wait for submitted GPU
work. It queries the fence/timeline value and retires completed work; explicit
fence waiting is used only for a `PollType::Wait`. Vulkan
`Fence::maintain` queries completed fences or the timeline semaphore and may
reset completed pooled fences, but it does not wait for the new submission.
Sources: pinned `wgpu-core/src/device/resource.rs::Device::maintain` and
`wgpu-hal/src/vulkan/mod.rs::Fence::{get_latest,maintain}`.

CuePool's own steady-state locks do not explain a read-versus-write queue stall.
`configure_gate` is held for reading by both consume and render work and excludes
only surface reconfiguration; `OutputFrameState` is held only for a short
snapshot/publication, outside GPU calls. Sources:
`examples/cuepool/crates/cuepool/src/video_pipeline::{video_consume_thread,output_render_thread}`
and `examples/cuepool/crates/cuepool/src/output_window::App::create_output_windows`.

The module comments introduced by `daa67a8` describe the reason for one FIFO
render thread per output: each thread should block on its own display rather
than serializing ungenlocked vsync waits. The comments introduced by `c90dd2d`
record the later Windows/NVIDIA observation that a non-render thread's Vulkan
GPU work stalled behind those WSI-blocked threads. Those comments are valuable
field history, not proof of a closed-source driver lock. The pinned source above
does prove the in-process route by which that observation can occur: acquire
holds the shared fence read guard across its Windows pacing waits, while submit
and present require the fence write guard.

### Proven versus rig-only

| Claim | Status | Evidence or required test |
| --- | --- | --- |
| Acquire and submit contend on the same fence RwLock, read versus write. | Proven from pinned source. | `Surface::get_current_texture` and `Queue::submit` in wgpu-core. |
| Present and submit are serialized by the same fence write lock. | Proven from pinned source. | `Surface::present` and `Queue::submit` in wgpu-core. |
| FIFO acquisition may hold the read guard while waiting in `vkAcquireNextImageKHR` and the Windows pacing `vkWaitForFences`. | Proven from pinned source. | `NativeSwapchain::acquire`. |
| Core submit has no intentional wait for the just-submitted GPU work. | Proven from pinned source. | `Device::maintain(Poll)` and Vulkan `Fence::maintain`. |
| Strict relay ordering and the single raw Vulkan queue serialize GPU submissions. | Proven from pinned source. | Vulkan `Queue::submit` and `RelaySemaphores::advance`. |
| The measured 14.8 ms is fence-lock wait rather than time in `vkQueueSubmit` or maintenance. | Not proven. | Needs the split below on ASHOF-PC12. |
| NVIDIA driver 596.49 serializes WSI calls across these three swapchains in a way that lengthens the lock hold. | Field hypothesis only. | Compare lock/driver splits, present modes, and preferably one qualified driver version. |
| Batching conversion into an output submit restores 50 delivered frames/s. | Plausible, not proven. | Only the three-output rig can establish it. |
| Convert-on-sample has enough shader and memory-bandwidth headroom on the wall. | Not proven. | GPU timing and show-length soak on the rig. |

The source therefore supports the contention mechanism, not the numerical
attribution. In particular, a long `Queue::submit` at the call site is not proof
that `vkQueueSubmit` itself took 15 ms.

### Minimal field diagnostic

Make one temporary, feature-gated diagnostic patch in the pinned wgpu fork and
report a 50-sample rolling mean plus one-second maximum in Help → Status:

1. In wgpu-core `Queue::submit`, time from immediately before
   `device.fence.write()` until the guard is acquired. Report it as
   `Convert submit lock wait ms`.
2. In the Vulkan HAL, time only the `vkQueueSubmit` call. Report it as
   `Convert vkQueueSubmit ms`.
3. Keep the existing outer `Conversion submit ms/frame`; derive
   `Convert core/other ms` as outer total minus the two measured buckets.

For the field build, bucket samples by the existing thread name
`video-consume`; render threads are already named `output-render-*`. This avoids
a permanent public wgpu API for one investigation. Lock-free atomics are enough
because there is one device and the Status snapshot is approximate diagnostics,
not control state. Sources for the thread names and current rolling window:
`examples/cuepool/crates/cuepool/src::{main::App::new,output_window::App::create_output_windows}`
and `examples/cuepool/crates/cuepool/src/video_pipeline::TimingWindow`.

The three rows answer the immediate decision:

- Large lock wait, small `vkQueueSubmit`: the fence/acquire/present interaction
  is the ceiling; batching should remove it from the consumer.
- Small lock wait, large `vkQueueSubmit`: run the present-mode/driver matrix and
  expect batching to move rather than remove the cost.
- Large `core/other`: split pending-write preparation from post-submit fence
  polling in a second build; do not guess further in the first patch.

If lock wait dominates but ownership is still ambiguous, add maximum hold times
around `Surface::get_current_texture`'s fence read guard and
`Surface::present`'s fence write guard. That is a follow-up, not part of the
minimal first diagnostic.

### Track A alternatives

#### Preferred cheap fix: batch conversion into an existing output submit

The consume thread should still upload the selected frame's planes and encode
the conversion command buffer, but hand the finished buffer to one output
thread instead of calling `queue.submit`. The first healthy output thread to
claim it submits conversion before its normal output command buffer in the same
call. Queue pending writes execute before the submitted command buffers, and the
command-buffer order makes conversion finish before that output samples the
canvas. Other outputs either show the previous canvas for that scanout or submit
after the conversion and show the new one; no output samples a half-converted
canvas. Sources for pending-write preparation and ordered command buffers:
pinned `wgpu-core/src/device/queue.rs::Queue::submit`; source for the current
split upload/encode API:
`examples/cuepool/crates/cuepool-video/src/yuv_converter::YuvConverter::{upload,encode}`.

Use a capacity-one handoff with epoch tagging and an acknowledgement after
`Queue::submit` returns. Do not let the consume thread overwrite the converter's
plane textures or uniform while a conversion buffer is waiting to be claimed.
If the handoff is occupied, keep newest-due semantics and account for the
discard. Any healthy output may claim the work so an occluded output 0 does not
freeze visible outputs. The existing output-0 present tick can remain the decode
pacer. Sources for epoch and pacing behaviour:
`examples/cuepool/crates/cuepool/src/video_pipeline::{VideoControl,frame_pacing_decision,video_consume_thread,output_render_thread}`.

For D3D11 NV12, batch the existing acquire, convert, and release command buffers
before the claimant's output buffer, attach the keyed-mutex operation exactly as
today, and register completion against that combined submit. The decoder lease
still retires only after submitted work completes. Sources:
`examples/cuepool/crates/cuepool/src/video_pipeline::video_consume_thread`,
`examples/cuepool/crates/cuepool-video/src/d3d11_zero_copy::D3d11Frame`, and
`examples/cuepool/crates/cuepool-video/src/frame_lease::SubmissionRetirement`.

This alternative removes one queue call per delivered frame, preserves one YUV
conversion per frame, leaves every canvas consumer untouched, and requires no
new frame-path submit. Its limitation is cross-output generation skew of up to a
refresh: ungenlocked outputs can observe the new canvas on adjacent scanouts.
The current architecture already makes no simultaneous-scanout guarantee.

#### Timing handoff around the present tick

The consume thread currently wakes after output 0 has submitted and presented;
that render thread can immediately loop into the next blocking acquire and take
the fence read guard before the consumer reaches submit. A handshake that holds
all output threads out of acquire until conversion submits would open a write
window, but it re-couples three independently paced outputs and risks undoing the
fix from `daa67a8`. A delay aimed only at output 0 does not exclude outputs 1 and
2. Sources:
`examples/cuepool/crates/cuepool/src/video_pipeline::output_render_thread` and
`examples/cuepool/crates/cuepool/src/output_window::OutputWindow`.

Do not ship this as the first fix. Batching gets the same queue-ordering benefit
without a new cross-output acquisition barrier.

#### Present-mode and driver matrix

`QPLAYER_PRESENT_MODE` already permits FIFO, FIFO-relaxed, Mailbox, and
Immediate, and non-FIFO modes are explicitly throttled because they free-run.
Use one short diagnostic run per supported mode and record the two submit timing
buckets plus upload/drop/present rates. A collapse in fence-lock wait outside
FIFO would confirm present/acquire interaction, but Mailbox or Immediate is not
the production answer: prior field work found free-running output pacing worse,
and tearing is unacceptable on the wall. Source:
`examples/cuepool/crates/cuepool/src/output_window::App::create_output_windows`.

If operationally safe, repeat FIFO on one other qualified NVIDIA driver. That
comparison can establish driver sensitivity, but source cannot predict it. A
backend change is not a cheap substitute because D3D11 zero-copy is specifically
implemented for Vulkan and would need a separate design.

## Track B: convert uploaded planes when each output samples

### Target architecture

CPU-readable YUV no longer writes the projection canvas. The consume thread
selects a frame, writes its planes into a bounded immutable slot, and atomically
publishes a content bundle. Each output's existing projection pass maps output
pixels through its source rectangle and the video's canvas-fit transform, samples
the planes, applies the current YUV matrix, composites the overlay, applies edge
blend and fade opacity, and writes its surface. Its one existing submit remains
the only submit for that output refresh.

```text
CPU YUV decode -> consume: select + write plane slot -> ProjectionContent::Planes
                                                        /       |       \
                                  existing output pass 0        1        2
                                  sample Y/U/V + fit/source rect + overlay
                                  edge blend/gamma/fade -> existing submit

RGBA image/text fallback -> RGBA canvas -----------------------> same outputs
text overlay --------------------------------------------------> same outputs

ProjectionContent::Planes or Canvas -> PixelSampler's existing submit -> lighting

D3D11 NV12 -> first healthy output's existing submit:
              acquire -> convert to RGBA canvas -> release -> composite
              other outputs continue to sample the RGBA canvas
```

There is no consume-thread submit for CPU-uploaded video, no extra output submit,
and no extra sampler submit. `PixelSampler` retains the submit it already needs
for downsample/readback, and the independent PixelMap cue pipeline retains its
existing YUV-to-pixmap submit; neither is a newly introduced projection-video
submission. Sources for those existing calls:
`examples/cuepool/crates/cuepool-video/src/pixel_sampler::PixelSampler::sample`
and `examples/cuepool/crates/cuepool/src/main::App::upload_pixmap_frames`.

### Published content and glitch-free mode changes

Replace the loose canvas fields and `has_content` flag in `OutputFrameState` with
one atomic base-content mode plus the existing independent overlay and projection
state:

- **Black**: no base image. With no overlay, outputs clear black; with a text
  overlay, the overlay is composited over black.
- **Canvas**: an RGBA canvas view for images, stills, and long-tail decoded
  formats that `video_source` sends through swscale as `FramePixels::Rgba`.
- **Planes**: an uploaded planar or NV12 bundle with epoch, generation, source
  dimensions, canvas-fit geometry, colour metadata, and owned texture views.
- **DirectCanvasPending/Canvas**: a D3D11 direct frame awaiting the next batched
  output submit, followed by the normal converted canvas.

The mutex-protected state publishes the base mode, overlay view, opacity,
identify flag, canvas size, and live output configurations as one generation.
Unlike today's shared mutable canvas, every accepted plane frame bumps the
generation. Output threads snapshot one complete generation and never combine a
new topology or fit uniform with old texture views. Source for the current loose
fields and change detection:
`examples/cuepool/crates/cuepool/src/video_pipeline::OutputFrameState`.

At a video-to-image boundary, bump `VideoControl::stream_epoch` first, stop
admitting old decode work, finish the image upload, then publish `Canvas` in one
state update. At image-to-video, retain the image until the first plane bundle is
fully written and publish `Planes` only then. Text remains an orthogonal overlay,
so starting or stopping text never changes the base mode. An overlay-only cue
publishes `Black` as its base rather than exposing the last video generation.
These rules preserve the intent of today's ordering in
`examples/cuepool/crates/cuepool/src/main::{App::play_cue,App::play_video,App::clear_text_overlay,App::stop_video_playback}`
and `examples/cuepool/crates/cuepool/src/video_pipeline::CanvasCommand`.

### Plane bundle ownership, slots, and epochs

The consume thread owns a lazily allocated three-slot plane pool, matching the
existing `VIDEO_QUEUE_CAP`. Each slot is an `Arc`-leased unit that owns its
`wgpu::Texture` handles and views. The published bundle and every output snapshot
hold a slot lease, so resize, format change, stop, or project reset cannot
destroy or overwrite textures still visible to a renderer. Allocate separate
slot shapes for planar R8, planar R16, and NV12 R8/RG8 as needed; never
reinterpret a slot across an incompatible topology. Sources for the channel
bound and current texture topology:
`examples/cuepool/crates/cuepool/src/main::VIDEO_QUEUE_CAP` and
`examples/cuepool/crates/cuepool-video/src/yuv_converter::{PlanarBinding,Nv12Binding}`.

At the start of each refresh, before surface acquisition, an output compares the
published generation, clones the new bundle lease, and drops its prior lease.
It keeps that lease through encoding and until `Queue::submit` returns. An
output blocked in FIFO acquisition therefore pins exactly the generation it
actually snapped, not every intervening generation. Removing an output joins
its thread before its leases can be ignored; a detached or wedged live thread
keeps its one slot quarantined rather than permitting an overwrite.

A slot is reusable only when it is no longer the published mode and the pool is
its sole remaining lease owner. No GPU completion wait is required for CPU
plane slots: the last renderer lease is dropped only after every old use held by
CuePool has entered the one strongly ordered queue, and the next `write_texture`
is flushed before a later output command buffer, so old reads precede the
overwrite. wgpu retains its own resource references for in-flight commands, and
the pinned Vulkan queue's relay semaphores make submission order explicit. If
no slot is safe, recycle the newly selected CPU frame and count a delivery drop;
never overwrite a possibly sampled slot. Sources: pinned
`wgpu-core/src/device/queue.rs::Queue::submit` and pinned
`wgpu-hal/src/vulkan/mod.rs::Queue::submit`.

`queue.write_texture` queues work; it does not require the consume thread to
submit. After all plane writes, the consume thread publishes the bundle. The
first later output submit flushes pending writes before its render command
buffer, so that output cannot sample partially uploaded planes. An output that
submitted before publication sees the prior generation. Thus the guarantee is
no within-output luma/chroma tearing and no mixed metadata; it is not simultaneous
generation changes across ungenlocked displays.

`stream_epoch` remains the authority for play, stop, seek/reopen, and project
changes. A consumed frame must match the current epoch immediately before plane
writes and again before publication. An epoch change drops unpublished CPU
frames immediately, publishes the replacement `Black` or `Canvas` mode, and
lets bundle leases make old slots reusable. Sources for current double epoch
checks:
`examples/cuepool/crates/cuepool/src/video_pipeline::{VideoControl,video_consume_thread}`.

### Shader organisation and colour parity

Use three projection pipelines, not one branch-heavy uber-shader and not one
pipeline per named pixel format:

1. RGBA canvas plus RGBA overlay: the current path.
2. Three planar textures plus overlay: 4:2:0, 4:2:2, and 4:4:4; R8 and R16 share
   the shader through `bit_depth_scale`.
3. Y plus interleaved UV plus overlay: uploaded NV12. The D3D11 path can reuse
   this sampling layout for its one convert-to-canvas pass.

Chroma subsampling needs no shader permutation because each plane texture has
its actual dimensions and normalized sampling already handles 420, 422, or 444.
The existing converter uses R8Unorm for 8-bit planes, R16Unorm with
`65535/1023` scaling for 10-bit planar data, and Rg8Unorm for NV12 chroma. It
implements limited versus full range and BT.601 versus BT.709. Move that fit and
matrix contract into one shared WGSL/Rust source used by the existing converter,
the projection shaders, and the pixel sampler; do not maintain three handwritten
matrix copies. Sources:
`examples/cuepool/crates/cuepool-video/src/{frame::FramePixels,yuv_converter::YuvConverter}`.

The decoder currently admits YUV420P/YUVJ420P, YUV422P/YUVJ422P,
YUV444P/YUVJ444P, NV12, and YUV420P10LE to the GPU path. `FramePixels` and
`YuvConverter` are structurally capable of planar 420/422/444 at either 8 or 10
bits, but `video_source::gpu_format_class` currently produces 10-bit only for
420P10LE. Other formats remain RGBA swscale fallback. Sources:
`examples/cuepool/crates/cuepool-video/src/video_source::{GpuYuvFormat,gpu_format_class,convert_frame}`
and `examples/cuepool/crates/cuepool-video/src/frame::{ChromaSubsample,BitDepth,FramePixels}`.

Preserve the logical canvas coordinate system. The fragment first maps output
coordinates through `ProjectorOutput`'s pixel-centred source rectangle to canvas
UV, applies Fit/Fill/Stretch to obtain source-plane UV or black letterbox, then
performs YUV conversion. Overlay sampling still uses the canvas UV. Finally the
shader applies each edge's smoothstep/gamma ramp and global stop-cue opacity.
Sources for current geometry/effects:
`examples/cuepool/crates/cuepool-video/src/{projection_renderer::Uniforms,shaders/projection.wgsl}`
and `examples/cuepool/crates/cuepool-video/src/yuv_converter::fit_rects`.

There is one easy colour trap. Today YUV conversion writes display-encoded RGB
bytes into a linear `Rgba8Unorm` canvas, and projection samples through an
`Rgba8UnormSrgb` view before doing edge blend in linear light. Direct projection
must therefore apply the equivalent sRGB decode to the matrix result before
overlay blend, edge blend, and opacity, then rely on the sRGB output surface to
encode it. The lighting sampler is different: it currently reads the canvas's
linear view specifically to recover the stored display-referred bytes, so its
plane shader must write matrix output directly without that sRGB decode.
Sources:
`examples/cuepool/crates/cuepool-video/src/{canvas_texture::CanvasTexture,projection_renderer::ProjectionRenderer,pixel_sampler::PixelSampler}`
and `examples/cuepool/crates/cuepool/src/main::App::about_to_wait`.

### Consumer inventory

| Consumer/state | Current behaviour | Track B behaviour |
| --- | --- | --- |
| Projection outputs | Each `ProjectionRenderer` samples the RGBA canvas and text overlay, applies the configured source rectangle, edge smoothstep/gamma, and global fade opacity. Source: `cuepool-video/src/{projection_renderer::ProjectionRenderer,shaders/projection.wgsl}`. | Select RGBA, planar, or NV12 pipeline from the atomic content mode. Plane pipelines do fit and YUV conversion before the same overlay, edge, and opacity operations. |
| Text overlay | Text is rasterized to RGBA and uploaded to a separate canvas-sized transparent texture; it may sit over video/image, or over an explicitly blank canvas. Source: `cuepool/src/main::{App::play_cue,App::rasterize_text_block}` and `cuepool/src/video_pipeline::CanvasCommand::Overlay`. | Keep the overlay texture and independent mode. It is sampled at logical canvas UV in every pipeline. Overlay-only publication uses a black base, preventing stale video at cue boundaries. |
| Image/still and RGBA fallback | Images stop the decode epoch and upload fitted RGBA into the canvas; unsupported decode formats are converted to `FramePixels::Rgba` by swscale. Source: `cuepool/src/main::App::play_cue`, `cuepool-video/src/video_source::convert_frame`, and `cuepool-video/src/canvas_texture::CanvasTexture::upload_frame`. | Retain the canvas and RGBA pipeline. Upload completes before one atomic `Canvas` publication. No plane pipeline is involved. |
| Windowed/operator preview | The current worktree has no live projection image embedded in the control window. `App::render_control` renders egui only, and `projection_panel::show` is configuration UI; unassigned output windows are the operator's projection preview and already use `ProjectionRenderer`. `cuepool_gui::preview` is an engine-free UI harness. Sources: `cuepool/src/main::App::render_control`, `cuepool/src/output_window::App::create_output_windows`, and `cuepool-gui/src/{projection_panel::show,preview}`. | No separate consumer to migrate. Windowed outputs take the same pipeline as fullscreen outputs. Do not invent a control-window video preview in this issue. |
| Lighting canvas pixel sampler | At lighting FPS, `PixelSampler` downsamples normalized canvas regions into small RGBA targets, submits one batch, and asynchronously reads it back for `LightingEngine::set_segment_pixels` and the control-window lighting grid. Sources: `cuepool/src/main::App::about_to_wait`, `cuepool-video/src/pixel_sampler::PixelSampler::{sample,collect}`, `cuepool/src/lighting_engine::LightingEngine::set_segment_pixels`, and `cuepool-gui/src/lighting_panel::segments_editor`. | Add the same RGBA/planar/NV12 topology variants to the sampler's existing pass. Apply canvas fit and YUV matrix before region downsampling; preserve display-referred output bytes. Reuse its existing submit, readback, one-frame latency, and in-flight skip rule. Until this stage lands, automatically use batched canvas conversion whenever an active segment names `SegmentSource::Canvas`. |
| Independent PixelMap cue source | A separate `pixmap_texture` and `pixmap_yuv` feed `SegmentSource::PixelMap`; its YUV path has its own submit on the winit thread. Source: `cuepool/src/main::{App::play_pixmap,App::upload_pixmap_frames}`. | Unchanged and explicitly out of scope. It is not the projection canvas or the failing consume-thread submit. |
| D3D11 NV12 direct path | The imported array exposes per-layer Y/UV views. One submission brackets acquire barrier, convert-to-canvas, release barrier, and keyed-mutex acquire/release; the decode thread waits for completion before retaking the pool. The first frame is compared with a readback canary. Sources: `cuepool-video/src/d3d11_zero_copy::{PoolInterop,D3d11Frame,D3d11Handoff}`, `cuepool-video/src/yuv_converter::YuvConverter::run_d3d11_canary`, and `cuepool/src/video_pipeline::{video_consume_thread,video_decode_thread}`. | Keep direct NV12 convert-to-canvas, but prepend acquire/convert/release to a healthy output's existing submit. This preserves one ownership interval and one conversion. Fold canary scratch renders/copies into that same existing submit; do not create a canary-only frame-path submit. |
| Recorder | The recorder captures DMX wire/OSC/MIDI input and pushes a lighting overlay; it neither reads nor records canvas pixels. It ticks after sampled pixels are published to the lighting engine, but those are a separate lighting layer. Sources: `cuepool/src/{recorder::Recorder::tick,main::App::about_to_wait}`. | Recorder code and semantics are unchanged. It benefits indirectly because canvas-source lighting segments keep receiving equivalent sampled RGB. |
| Identify | Identify bypasses content and clears each output to a per-output colour. Source: `cuepool/src/video_pipeline::output_render_thread` and `cuepool/src/main::App::about_to_wait`. | Keep identify as the highest-priority render mode. Plane uploads may continue; leaving identify displays the newest published generation without a special conversion. |
| Fade, stop, blank, and EOF | Stop-cue opacity dims canvas and overlay. Stop clears active content; OneShot EOF blanks canvas so an active text overlay cannot reveal the last clip; HoldLast and MTC hold the last frame. Sources: `cuepool/src/main::{App::stop_video_playback,App::stop_all,ApplicationHandler::user_event}` and `cuepool/src/video_pipeline::{video_consume_thread,output_render_thread}`. | Opacity remains after base/overlay composition. Stop and OneShot atomically publish `Black`; HoldLast/MTC retain the last plane bundle and its slot until the hold ends. Black never means "reuse last plane textures." |

### D3D11 direct path ([#118](https://github.com/BlueJayLouche/rustjay-engine/issues/118)) decision

Do not fan one imported decoder surface across three independent output submits
in this issue. The imported keyed mutex and external queue-family ownership are
resource-wide, while the current safe contract acquires and releases them in one
ordered submission and blocks decoder progress until completion. A true
direct-on-sample version would need a participant mask, a first-output acquire,
middle submissions that keep Vulkan ownership, a last-output release, timeout
recovery when an output is occluded/lost, and decoder retirement after the last
GPU completion. It would pace decode by the slowest output and turn an optional
optimization into the highest-risk part of the renderer. Sources:
`examples/cuepool/crates/cuepool-video/src/d3d11_zero_copy::{PoolInterop::record_barrier,D3d11Frame::attach_keyed_mutex,D3d11Handoff::wait}`.

Keeping one D3D11 conversion is acceptable because batching removes the 15 ms
dedicated submit while retaining zero-copy's avoided GPU-to-CPU transfer. The
RGBA canvas already exists for stills and overlays, and all secondary consumers
remain correct. If field GPU timestamps later show that this one conversion pass
itself is the zero-copy ceiling, direct multi-output ownership needs its own
design and attended NVIDIA qualification; it must not be smuggled into this
change.

The canary must still gate first use. Put its direct/readback scratch renders and
copies ahead of a regular output command buffer, but have that output render the
previous canvas; map and compare the canary result after the combined submission
completes, and enable direct conversion only on the next decoder frame if it
matches. A failed canary therefore cannot reach a projector. This costs one
startup frame and preserves the current conservative fallback without a
canary-only submit. Source for the current synchronous first-frame gate:
`examples/cuepool/crates/cuepool-video/src/yuv_converter::YuvConverter::run_d3d11_canary`.

### Memory cost

Let `C = canvas_width * canvas_height` and let `P` be one source plane set:

- RGBA canvas: `4C` bytes.
- RGBA overlay: another `4C` bytes.
- 8-bit 420 or NV12: `1.5 * source_width * source_height` bytes.
- 8-bit 422: `2 * source_width * source_height` bytes.
- 8-bit 444 or 10-bit 420 stored in R16 planes: `3 * source_width * source_height` bytes.
- A structurally possible 10-bit 422/444 set would be 4/6 bytes per source
  pixel, although `video_source` does not currently admit those formats.

Today the uploaded path normally owns two RGBA canvas-sized textures plus one
active YUV plane set. A three-slot design changes `8C + P` to `8C + 3P`, a
bounded increase of `2P`; retaining the canvas avoids allocations and flashes at
image/text/video boundaries. For a 5120x1200 source, one 8-bit 420/NV12 set is
8.79 MiB, so the three-slot delta is 17.58 MiB. One 8-bit 444 or 10-bit 420 set
is 17.58 MiB, so the delta is 35.16 MiB. These are logical texel sizes and
exclude allocator alignment, staging buffers, swapchains, and the externally
owned D3D11 decoder pool. Sources for current allocations:
`examples/cuepool/crates/cuepool-video/src/{canvas_texture::CanvasTexture,yuv_converter::YuvConverter}`
and `examples/cuepool/crates/cuepool/src/video_pipeline::video_consume_thread`.

Do not grow the slot pool dynamically. If three slots are insufficient, the
status counter and rig capture should reveal it; the safe response is a dropped
delivery, not unbounded show-time GPU memory.

### Counter meanings after the change

| Status value | Meaning after batching/convert-on-sample |
| --- | --- |
| Uploads/s | Source frames successfully staged and published for projection. An uploaded-plane frame counts after all `write_texture` calls and atomic bundle publication even though the consume thread did not submit. A direct frame counts after its conversion batch is accepted by an output submit. The label stays for field-series continuity. |
| Dropped/s | Due source frames not published: existing newest-due collapse plus a frame rejected because the conversion handoff or all plane slots were busy. Preserve one total, but add a nonzero-only reason such as `plane slots busy` to logs/Status so backpressure is distinguishable. |
| Starved/s | Unchanged: a frame was due while the active decode receiver existed but supplied neither a current nor peeked frame. Source: `video_pipeline::video_consume_thread`. |
| Presented/s | Unchanged per output: successful render-thread present calls, independent of whether the output repeated or skipped a content generation. Source: `video_pipeline::output_render_thread`. |
| Upload ms/frame | Plane or RGBA queue writes and publication work; it no longer includes a conversion submit. |
| Conversion submit ms/frame | In sample mode display `n/a (sampled in output submit)`, not a misleading zero. In batched/direct-canvas mode report handoff-to-combined-submit latency separately from the Track A lock/vk-call split. |

The important invariant remains `Uploads/s + Dropped/s` approximately equal to
the due source cadence when starvation is zero. Presented/s measures output
pacing and is not expected to equal Uploads/s times output count. Current
publication sources are
`examples/cuepool/crates/cuepool/src/main::App::about_to_wait` and
`examples/cuepool/crates/cuepool-gui/src/app::Diagnostics::sections`.

## Verification and staged implementation

Every stage is independently mergeable and leaves a working established
fallback.

1. **Submit split diagnostics.** Add the temporary wgpu-core fence-lock timer,
   Vulkan `vkQueueSubmit` timer, and Status rows. Host tests assert accumulator
   reset/snapshot arithmetic and that `core/other` never underflows. On the rig,
   capture FIFO baseline plus supported present-mode comparisons. Gate: the
   three buckets account for the existing outer submit time closely enough to
   choose lock versus driver follow-up. This stage changes no rendering.
2. **Combined render submit.** Hand one epoch-tagged conversion batch to the
   first healthy output and prepend it to that output's existing submit. Keep
   all canvas consumers and current shaders. Unit-test capacity-one newest-due
   handling, stale epoch rejection, claimant loss, and exactly-once direct lease
   retirement. A GPU test compares separate versus combined command-buffer
   ordering. Rig gate: 48–50 uploads/s, no sustained slot/handoff drops,
   Starved/s 0, each output 47–50 presents/s, and no new visual or direct-canary
   fault for at least one show-length soak. If it passes, stop here.
3. **CPU plane publication and projection sampling, opt-in.** Add the three
   bounded leased slots, atomic `Black/Canvas/Planes` mode, per-frame generation
   snapshots, and RGBA/planar/NV12 projection pipelines. Keep batched canvas
   automatically for D3D11 direct and whenever an active lighting segment uses
   the projection canvas. GPU golden tests compare the old converter-plus-canvas
   result with direct projection for 420/422/444 8-bit, 420 10-bit, and NV12;
   full/limited range; BT.601/709; Fit/Fill/Stretch; source crop; overlay alpha;
   edge gamma; opacity; and black letterbox. Permit only the rounding tolerance
   established by the existing RGBA path. State-machine tests cover video/image,
   video/text, stop, EOF HoldLast/OneShot, seek epoch, output removal, occlusion,
   and slot exhaustion. Gate: no standalone conversion submit in CPU-plane mode
   and no stale/mixed-generation frame in stress tests.
4. **Lighting sampler parity.** Add Canvas/planar/NV12 variants to
   `PixelSampler`'s existing batch and remove the CPU-plane lighting fallback.
   Compare sampled grid bytes against the current linear canvas view for every
   supported topology, range/matrix, fit mode, and segment region. Gate:
   identical fixture-cell ordering and colour within the established byte
   tolerance, unchanged async/in-flight behaviour, and no additional submit.
5. **D3D11 and rollout qualification.** Keep direct D3D11 conversion batched in
   an output submit and fold the first-frame canary work into a submit that still
   displays the previous canvas; admit direct content only after the readback
   comparison passes. Test direct acceptance, canary mismatch, keyed-mutex
   timeout, output claimant loss, epoch change, and fallback to D3D11VA readback.
   Then run the venue's
   shipping files through pause, step, seek, loop, rapid video/image/text
   boundaries, identify, StopAll, OneShot, HoldLast, MTC hold, Canvas-source
   lighting, projector loss/rebuild, and show-length soak. Only the Windows
   NVIDIA/Vulkan rig can prove driver synchronization, 50 fps delivery, colour
   on the projectors, bounded memory/handles, and absence of long-tail tearing.

The conversion-submit call count should be observable in test/diagnostic builds:
zero standalone projection-video conversion submits after stage 2, one existing
submit per presented output refresh, and only the pre-existing conditional pixel
sampler and independent PixelMap submits.

## Risks and rollback

| Risk | Failure | Containment |
| --- | --- | --- |
| Plane slot reused while an output can still encode the old generation | Mixed metadata, wrong cue, torn luma/chroma, or validation/device loss | Three bounded slots; bundle leases held through submit; no overwrite on uncertainty; epoch and occlusion/removal tests. |
| Direct projection changes the current sRGB round trip | Washed, dark, or incorrect blend bands despite correct YUV matrix | Explicit encoded-to-linear step in projection only; byte and rendered-surface parity tests; attended wall comparison. |
| Batching only moves a long driver call onto an output thread | One output loses present cadence while consumer improves | Track A split first; claimant can be any healthy output; per-output Presented/s is the gate; rollback to current submit with one switch. |
| Plane ring exhausts under an occluded or wedged output | Drops rise despite decode headroom | Refresh the slot lease before acquire, quarantine detached threads, bounded drop with reason, retain batched canvas fallback. |
| D3D11 lease is released before batched work completes | Decoder/Vulkan race, corruption, or hang | Keep current completion retirement and one acquire/convert/release ownership interval; no multi-output direct sampling. |
| Integrated shader cost exceeds GPU headroom | Presented/s falls although uploads recover | GPU timestamps and rig soak; revert CPU YUV to combined canvas submit without changing cue or canvas semantics. |

The single biggest Track B risk is lifetime correctness across three independently
paced outputs: one stale renderer must never be allowed to submit a bundle after
its plane slot has been overwritten. The fixed pool, lease-through-submit
contract, and drop-on-uncertainty rule are load-bearing.

Rollback is cheap and carries no data migration. Stage 2 can restore the current
standalone submit; Track B can route CPU plane formats back through the combined
canvas conversion while leaving images, overlays, lighting, D3D11 fallback, cue
state, and show files unchanged.
