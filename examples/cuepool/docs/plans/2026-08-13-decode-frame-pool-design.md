# Decode frame pooling and timing design

## Buffer ownership

`cuepool-video` exposes a std-only `FramePool` backed by
`Mutex<Vec<Vec<u8>>>`; callers share it through `Arc<FramePool>`. The pool lends an empty `Vec<u8>` to each decoded
RGBA or YUV plane copy and accepts the allocation back after GPU upload or
when a newer due frame supersedes it. `VideoFrame::into_buffers` consumes a
frame and returns only its pixel allocations, leaving its public data shape
unchanged.

One pool is shared by projector video and pixel-map video. It retains at most
30 buffers: two streams, five in-flight frames per stream
(`VIDEO_QUEUE_CAP + 2`), and three planes per frame. A return beyond the bound
drops normally. Reuse is stream-, resolution-, and format-agnostic because
each checkout clears the vector before copying the new bytes.

## Decode integration

`VideoSource::open_with_pool` accepts the shared pool while the existing
`VideoSource::open` remains available for examples and other callers. Hardware
fallback reopen preserves the source's pool. Both native YUV plane copies and
the swscale RGBA fallback draw from it.

The projector consumer recycles buffers only at existing ownership endpoints:
superseded due frames, stale frames, uploaded frames, and dropped peeked
frames. The pixel-map drain likewise recycles superseded and uploaded frames.
Thread teardown may drop frames without returning them; freeing those buffers
is safe and keeps stopped streams from extending retention.

## Timing diagnostics

The decode thread measures wall time around every `read_frame()` call. A fixed
50-entry ring stores milliseconds and publishes its arithmetic mean through a
new `VideoDiagnostics::decode_ms_per_frame` field alongside `decode_path`.
This includes demux, decode, hardware download, and conversion, but excludes
bounded-channel backpressure and presentation pacing.

## Checks

Unit tests cover empty-pool fallback, reuse across different payload sizes,
and retained-buffer bound enforcement. Existing cargo check, clippy, and test
gates cover the integration and diagnostics plumbing.
