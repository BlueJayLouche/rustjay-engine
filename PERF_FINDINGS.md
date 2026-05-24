# Performance Findings

**Date:** 2026-05-19  
**Method:** samply CPU profiler (macOS), 49.5s idle session  
**Target:** `examples/sputnik` — mesh + LFO, no video input, 120fps on ProMotion display  
**Tooling added:** `[profile.profiling]` in `Cargo.toml` (inherits release, `debug = 1`)

Each task below is self-contained: file paths, line numbers, what to change, and acceptance criteria. Tasks are independent unless noted.

---

## Measured Baseline

| Metric | Value |
|--------|-------|
| Total idle CPU (Activity Monitor) | **~25%** |
| Main thread CPU | 9.8% of one core |
| Metal worker threads (6×) | ~13% of total |
| Main thread active work per frame | ~381 µs |
| Frame rate | 120fps (ProMotion display) |

**Main thread breakdown:**

| Activity | CPU share | Notes |
|----------|-----------|-------|
| `get_current_texture()` vsync wait | 53% | GPU-paced — expected, not burnable |
| wgpu command encoding + submit | ~11% | Normal render overhead |
| Staging buffer allocation | ~5% | **Fixable — see Task P-1** |
| ImGui control window render | ~4% | Managed by imgui-wgpu |
| Everything else (audio, LFO, MIDI…) | ~27% | Below profiler resolution individually |

---

## Root Causes

### R-1 — Metal worker thread wake-ups (13% CPU)

Six wgpu-hal Metal worker threads wake on every GPU command completion. At 120fps they produce ~1,920 kernel wake-up events per second. Call stack:

```
pthread_cond_wait [libsystem_pthread]
  IOUserClient     [IOKit]
    IOGPU          [IOGPU]
```

This is intrinsic to wgpu's Metal backend. **The only lever we control is frame rate**: dropping to 60fps halves the wake-up rate and cuts this component to ~6–7%.

### R-2 — Per-frame staging buffer allocation (5% of main thread)

`wgpu::Queue::write_buffer` allocates a new `StagingBuffer` via `wgpu_hal::DynDevice::create_buffer` on every call. This fires each frame in two places:

| Call site | File:Line | Path |
|-----------|-----------|------|
| Uniform buffer update | `crates/rustjay-render/src/plugin_renderer.rs` | `engine.render()` at `events.rs:337` |
| ImGui vertex/index upload | inside `imgui-wgpu` | control window at `events.rs:351` |

The fix for the uniform buffer path is `wgpu::util::StagingBelt` (see Task P-2). The ImGui path is inside the `imgui-wgpu` dependency and requires an upstream fix or a fork.

### R-3 — Pre-profile suspects: not hot in practice

The following were identified by static analysis as per-frame allocations. They are real but fall below the 1ms sampling floor — they do not appear meaningfully in the profile and are not worth optimising ahead of R-1 and R-2.

| Code | File:Line | Why not a priority |
|------|-----------|-------------------|
| `custom_params.clone()` | `update.rs:134` | Sub-ms, invisible at scale |
| `HashMap::new()` for MIDI dirty values | `update.rs:204` | Same |
| `format!()` for OSC/web addresses | `update.rs:285, 311` | Same |
| ~21 mutex locks/frame on `shared_state` | various `update.rs` | No contention detected |

---

## Tasks

### Task P-1 — Make frame rate cap configurable (biggest win)
**File:** `crates/rustjay-engine/src/app/events.rs:357`  
**Effort:** 30 min  
**Expected gain:** ~6–8% CPU reduction at 60fps on a 60Hz display

**Problem:** The frame cap is a compile-time constant at 120fps:
```rust
const TARGET_FRAME_DUR: std::time::Duration = std::time::Duration::from_micros(8333);
```
On a 60Hz display or any display where the user does not need 120fps, this causes double the Metal command submissions and double the GPU driver wake-ups with no visual benefit.

**Fix:** Read the cap from `EngineConfig` (or expose it as a runtime field on `App`) so callers can pass 60fps for non-ProMotion contexts. The `sputnik`, `delta`, and `waaaves` examples can default to 60fps; ProMotion users can opt up.

```rust
// In App / EngineConfig, add:
pub fps_cap: u32,  // default 60

// In about_to_wait:
let target = Duration::from_secs(1) / self.config.fps_cap;
```

**Acceptance:** Running `sputnik` with `fps_cap: 60` on a 60Hz display shows ≤14% idle CPU in Activity Monitor (from ~25%).

---

### Task P-2 — Replace `write_buffer` with `StagingBelt` in plugin renderer
**File:** `crates/rustjay-render/src/plugin_renderer.rs`  
**Effort:** 1–2 hrs  
**Expected gain:** ~5% of main thread CPU (staging buffer allocation gone)

**Problem:** Every frame, uniform buffer updates call `queue.write_buffer(...)` which allocates and immediately destroys a `StagingBuffer`. `StagingBelt` pools these buffers and recalls them after GPU submission.

**Fix outline:**
1. Add `wgpu::util::StagingBelt` as a field on `WgpuEngine` (or `PluginRenderer`), initialized with a chunk size of e.g. 4096 bytes.
2. Replace every `queue.write_buffer(&self.uniform_buffer, ...)` call with:
   ```rust
   belt.write_buffer(&mut encoder, &self.uniform_buffer, 0, NonZeroU64::new(size).unwrap(), device)
       .copy_from_slice(bytemuck::bytes_of(&uniforms));
   ```
3. Call `belt.finish()` before `queue.submit(...)`.
4. Call `belt.recall()` after submission (or in the next frame's start — `StagingBelt::recall` is async-safe with `pollster`).

**Acceptance:** `StagingBuffer::new` no longer appears in a samply flamegraph of the render path.

---

### Task P-3 — Audit `imgui-wgpu` staging buffer usage (low priority)
**Dep:** `imgui-wgpu = "0.28"` in `Cargo.toml`  
**Effort:** investigation only  
**Expected gain:** unknown until audited

**Problem:** The ImGui control window render path (at `events.rs:351`) also triggers `StagingBuffer::new` via `imgui-wgpu`'s internal `write_buffer` call for vertex/index data upload. This fires even when the UI has no visible changes.

**Action:** Check whether `imgui-wgpu` 0.28 has a `StagingBelt` option, or whether there is a more recent version / alternative crate that avoids per-frame allocations. If not, consider upstreaming a fix or caching vertex data when the frame is not dirty.

---

## How to Re-Profile

```bash
# Build with debug symbols (profiling profile added to Cargo.toml)
cargo build --profile profiling -p sputnik

# Record with samply — close the window after ~30s idle
samply record ./target/profiling/sputnik

# samply opens Firefox Profiler automatically in your browser
# Use flame graph view, filter to "sputnik" thread, sort by CPU delta
```

To compare before/after a fix, save the samply profile JSON and diff the `threadCPUDelta` totals.
