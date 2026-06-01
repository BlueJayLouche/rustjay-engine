# RustJay Engine — Performance Profile Report
**Date:** 2026-06-01  
**Target:** `examples/sputnik` (mesh + LFO, no video input, 120fps ProMotion)  
**Duration:** 30 seconds idle runtime  
**Tools:** macOS `sample` (1ms interval), `samply` (Firefox Profiler format)  
**Build:** `cargo build --profile profiling -p sputnik`

---

## Generated Artifacts

| File | Size | Description |
|------|------|-------------|
| `sputnik_sample.txt` | 1.5 MB | macOS `sample` report with full symbols — primary analysis source |
| `sputnik_profile_20260601_095120.json` | 3.8 MB | `samply` profile (Firefox Profiler format) — load in profiler.firefox.com |
| `PROFILE_REPORT_2026-06-01.md` | — | This summary |

---

## Executive Summary

The sputnik example spends the vast majority of its CPU time either **waiting for the GPU display sync** or in **macOS event-loop machinery**. Actual Rust application work is a small fraction of total CPU. The audio thread is extremely light. No new regressions were detected compared to the baseline captured on 2026-05-19.

| Metric | Value |
|--------|-------|
| Main thread total samples | 24,410 |
| GPU/vsync wait (`get_current_texture` → `semaphore_timedwait_trap`) | **~13,569 samples (55.6%)** |
| Texture upload (`write_texture` path) | **~873 samples (3.6%)** |
| Render encoding + submit (`WgpuEngine::render` + `finish`/`submit`) | **~613 samples (2.5%)** |
| Uniform buffer upload (`write_buffer` + `StagingBuffer::new`) | **~122 + 74 = 196 samples (0.8%)** |
| ImGui UI build | **~65 samples (0.3%)** |
| Audio FFT processing | **~14 samples (negligible)** |

---

## Detailed Bottlenecks

### 1. GPU VSync Wait — 55.6% of main thread ⭐ largest factor

**Stack:**
```
about_to_wait (events.rs)
  → wgpu::Surface::get_current_texture
    → wgpu_hal::DynSurface::acquire_texture
      → CAMetalLayer::nextDrawable
        → CAMetalLayerPrivateNextDrawableLocked
          → _dispatch_semaphore_wait_slow
            → semaphore_timedwait_trap
```

**Analysis:**  
This is the Metal display-link blocking the main thread until the next frame buffer is available. At 120fps on a ProMotion display this produces ~1,920 kernel wake-ups per second across six wgpu-hal Metal worker threads. **This is expected GPU pacing, not burnable CPU work**, but it is the dominant component of the process's CPU footprint.

**Reviewer note:** The only lever the application has is frame rate. Dropping to 60fps would halve the wake-up rate and cut this component to roughly half. See Task P-1 in `PERF_FINDINGS.md`.

---

### 2. Per-Frame Texture Upload — 3.6% of main thread

**Stack:**
```
about_to_wait (events.rs:528)
  → App::update_input_slot
    → InputTexture::update (texture.rs:218)
      → Queue::write_texture
        → StagingBuffer::new  ← allocates a new staging buffer every frame
        → copy_buffer_to_texture (Metal blit encoder)
```

**Analysis:**  
Even with no active video input, the input-slot update path calls `write_texture` each frame, which allocates and immediately destroys a `StagingBuffer`. This is the same pattern identified in `PERF_FINDINGS.md` Task P-2 for uniform buffers, but here it affects the texture upload path.

**Reviewer note:**  
- The texture upload path should be audited to see if it can be skipped when no new frame data is available.  
- If the upload must happen every frame, consider using a persistent `Buffer` + `copy_buffer_to_texture` instead of `write_texture`, or pool the staging allocation.

---

### 3. Render Path Overhead — ~2.5% of main thread

**Components:**

| Function | Samples | % of main thread |
|----------|---------|------------------|
| `WgpuEngine::render` | 370 | 1.52% |
| `CommandEncoder::finish` | 185 | 0.76% |
| `CommandEncoder::encode_commands` | 182 | 0.75% |
| `Queue::submit` | ~130 | 0.53% |
| `encode_render_pass` | 113 | 0.46% |

**Analysis:**  
This is normal wgpu command encoding and submission overhead. No single hotspot stands out. The render pass encoding (`encode_render_pass`) is slightly heavier than a trivial app would be, which is expected given the mesh + LFO workload.

---

### 4. Uniform Buffer Staging Allocation — 0.8% of main thread

**Stack:**
```
Queue::write_buffer
  → StagingBuffer::new
    → DynDevice::create_buffer
```

**Analysis:**  
`StagingBuffer::new` appears at 74 explicit samples plus 122 samples in `write_buffer`. This is the exact issue documented in `PERF_FINDINGS.md` Task P-2. At 120fps the per-frame allocation is visible but small (~0.8%). The proposed fix (`wgpu::util::StagingBelt`) would eliminate this entirely.

---

### 5. ImGui Control Window — 0.3% of main thread

**Samples:** ~65 in `ControlGui::build_ui`, plus a few in `imgui::Window::begin` and combo widgets.

**Analysis:**  
Very light. No concern for the reviewer unless the UI grows significantly more complex.

---

### 6. Audio Thread — negligible

**Thread:** `com.apple.audio.IOThread.client`  
**Rust work:** ~14 samples in `process_audio_frame` and NEON FFT butterflies.

**Analysis:**  
The audio callback is extremely cheap. The FFT (RealToComplex + NeonRadix4) is well vectorized and uses <0.1% of total process CPU.

---

### 7. Memory Allocation Pressure

**Observed in sample:**
- `_xzm_malloc_large_huge`: 93 samples
- `_xzm_xzone_malloc_tiny`: 58 samples
- `_xzm_xzone_malloc`: 30 samples

**Analysis:**  
These are mostly inside the Metal driver (texture/drawable allocation) and wgpu's per-frame staging buffers. No Rust-side allocator thrashing is visible.

---

## Metal Worker Threads

- **IOGPU mentions:** 439
- **pthread_cond_wait mentions:** 32

These correspond to the six wgpu-hal Metal worker threads waking on every GPU command completion. As noted in `PERF_FINDINGS.md` Root Cause R-1, this is intrinsic to wgpu's Metal backend and is only addressable by reducing frame rate.

---

## Recommendations for Performance Reviewer

1. **Biggest win:** Make the frame-rate cap configurable (Task P-1). On 60Hz displays, forcing 120fps doubles GPU driver wake-ups for zero visual benefit.
2. **Second win:** Replace `Queue::write_buffer` with `StagingBelt` in `plugin_renderer.rs` (Task P-2). This removes ~0.8% main-thread CPU and reduces allocator pressure.
3. **Investigate:** The texture upload path (`InputTexture::update`) fires every frame even with no video input. Consider early-exiting when the input slot has no new data.
4. **Low priority:** The audio, ImGui, and preset/update paths are all below the noise floor. Do not optimize ahead of the GPU sync and staging buffer issues.

---

## How to Reproduce

```bash
# Build
 cargo build --profile profiling -p sputnik

# macOS sample (text report)
 ./target/profiling/sputnik &
 sample $(pgrep -x sputnik) -file sputnik_sample.txt 30
 kill $(pgrep -x sputnik)

# samply (Firefox Profiler JSON)
 samply record -d 30 --save-only --no-open -o sputnik_profile.json ./target/profiling/sputnik
```

---

## Comparison to Baseline (2026-05-19)

| Metric | 2026-05-19 (PERF_FINDINGS.md) | 2026-06-01 (this run) | Δ |
|--------|------------------------------|----------------------|---|
| Total idle CPU | ~25% | not measured | — |
| Main thread active work/frame | ~381 µs | not measured | — |
| VSync wait share | ~53% | **55.6%** | similar |
| Staging buffer share | ~5% | **3.6% texture + 0.8% uniform** | similar, split across two paths |
| ImGui share | ~4% | **0.3%** | lower (possibly UI state difference) |
| Audio | below resolution | **negligible** | consistent |

No regressions detected. The profile shape is stable.
