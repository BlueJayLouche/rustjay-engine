# D3D12VA zero-copy design (#110 replacement path)

Replaces the experimental D3D11VA→Vulkan keyed-mutex interop (#114 design,
#118 implementation) with FFmpeg D3D12VA decoding consumed directly by wgpu's
DX12 backend on one shared `ID3D12Device`. Qualification target is the
NVIDIA adapter in ASHOF-PC02; Intel qualification was dropped by direction on
2026-08-15 (the UHD 630 is BIOS-disabled since the RTX 5050 install and is no
longer a venue target).

## Device sharing

wgpu owns the `ID3D12Device` (DX12 backend). FFmpeg adopts it:
`av_hwdevice_ctx_alloc(AV_HWDEVICE_TYPE_D3D12VA)` → set
`AVD3D12VADeviceContext.device` to wgpu's raw device →
`av_hwdevice_ctx_init`. FFmpeg's `d3d12va_device_init` queries its own
`ID3D12VideoDevice` and creates its own decode command queue; decode work never
touches wgpu's direct queue.

This direction is mandatory, not stylistic: FFmpeg's D3D12VA pool allocates
per-frame committed resources with `D3D12_HEAP_FLAG_NONE` (not shareable), so
cross-device NT-handle export is impossible without patching FFmpeg. Same
device or nothing.

## Frames-context configuration (and the upstream FFmpeg bug)

As with the D3D11 path's `configure_pool`, the decoder's `get_format` callback
allocates the hardware frames context itself via
`avcodec_get_hw_frames_parameters(…, AV_PIX_FMT_D3D12, …)`, then adjusts it
before `av_hwframe_ctx_init`:

- **Dimensions are forced to the coded size.** Upstream FFmpeg (8.0/8.1/master)
  sizes the D3D12 pool and decoder heap from the *display* dimensions
  (`ff_d3d12va_common_frame_params` uses `avctx->width/height`), while HEVC
  decodes at *coded* size. Any stream whose 16-aligned display height is
  smaller than its coded height fails on NVIDIA ("hardware accelerator failed
  to decode picture"). The venue 5120×1200 masters code 1216 rows with a
  16-row conformance crop and hit this exactly. Root-caused empirically on the
  RTX 5050 (driver 610.88) with boundary tests (1076→pass / 1072→fail /
  1168→fail) and confirmed against the FFmpeg source; a Rust probe using the
  production `ffmpeg-next` crate decodes 100/100 frames at 562 fps once the
  frames context is forced to coded dimensions, and 0 frames without.
- **Pool size is expanded** for CuePool's in-flight lease budget, mirroring
  `expanded_pool_size` on the D3D11 path (D3D12VA has no texture-array axis
  limit; per-frame committed resources).
- sw_format must be NV12; anything else declines to the readback path.

## Per-frame flow

```
FFmpeg decode (own queue)          wgpu DX12 (direct queue)
  AVFrame: AVD3D12VAFrame            import texture (same device, raw resource)
    .texture: ID3D12Resource   →     Plane0 R8 / Plane1 RG8 views
    .sync_ctx.fence + value    →     queue Wait(fence, value)  ┐ atomic pair
                                     convert submission        ┘ (serialized)
                                     transition back to COMMON
  AVFrame lease held until wgpu submission completes
```

- Each decoded frame carries its own `(ID3D12Fence, fence_value)`; values
  increment per pool-slot reuse, so the wait must bind the exact pair captured
  with the frame — never "latest".
- Imported textures are cached keyed on the `ID3D12Resource` pointer (the pool
  reuses a bounded set of resources; views are created once per resource).
- Resources are created in `D3D12_RESOURCE_STATE_COMMON` and FFmpeg's decoder
  transitions them COMMON→VIDEO_DECODE_WRITE→COMMON per decode, so the
  consuming submission must leave them in COMMON again after sampling.
- The AVFrame reference (lease) is what parks the pool slot: FFmpeg cannot
  reuse the resource until the lease drops, and the lease drops only after the
  consuming wgpu submission is observed complete. Lease budget and poisoning
  containment carry over from the D3D11 path unchanged.

## Submission serialization

CuePool submits wgpu work from several threads (consume thread, per-output
render threads, egui/main). `ID3D12CommandQueue::Wait` gates *everything*
executed after it on the queue timeline, so a wait must be immediately followed
by its own submission with no interleaving submit from another thread —
otherwise the wait attaches (as an ordering hazard and a latency tax) to
unrelated work. All submissions therefore go through one shared serialized
queue wrapper; the zero-copy path acquires it for the atomic
wait+submit(+state-release) pair, every other submit site acquires it for plain
submits. A focused unit test asserts the association invariant (no submission
can interleave a wait pair) at the wrapper level.

## Opt-in, fallbacks, surfaces

- `QPLAYER_ZEROCOPY` remains the only switch, same semantics
  (`ZeroCopyPreference`), no new env vars, options, deps, or public API.
- D3D11VA readback and software decode fallbacks are untouched; any decline
  (feature missing, non-NV12, import failure, canary failure, poisoned path)
  reports a reason string exactly like today and falls back.
- Colour metadata (range/matrix) flows into the existing NV12→RGB conversion
  unchanged; startup canary and tolerance unchanged.
- `decode_path` reports "D3D12VA zero-copy" plus the adapter name.
- The wgpu fork drops the Vulkan external-memory/keyed-mutex patches; only the
  frame-pacing diagnostics remain as a fork patch on wgpu 30 (plus the fence
  lock/acquire split if not upstreamed).

## Qualification

5120×1200@50 HEVC project, cue 2, Immediate, three outputs; 60 s warm-up +
10 min soak on the RTX 5050. Pass gates: decode_path identifies D3D12VA
zero-copy on the intended adapter; canary passes; uploads mean 49–51/s with
≥95% of samples 48–52/s; drops ≤0.1/s without sustained bursts; zero
starvation/consumer errors; decode sync + conversion + upload within the 20 ms
frame budget; clean shutdown with all leases released.
