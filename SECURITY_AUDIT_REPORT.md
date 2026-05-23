# RustJay Engine — Comprehensive Security & Stability Audit Report

**Date:** 2026-05-23  
**Scope:** Entire codebase (crates/, examples/, vendor/)  
**Focus:** Security, stability, null safety, deadlocks, livelocks, logic errors, control-flow correctness  

---

## Second-Opinion Review — 19-Commit PR (2026-05-23)

**Reviewer:** Independent three-phase analysis (identification + parallel false-positive filtering)  
**Scope:** PR diff only — changes introduced by the 19 commits not yet pushed to `origin/main`

### Verdict: No New High-Confidence Vulnerabilities

After a three-phase review (identification → parallel false-positive filtering), **no security vulnerabilities above the reporting threshold were introduced by this PR**. Two candidates were evaluated and eliminated:

| Candidate | Outcome | Reason |
|-----------|---------|--------|
| `serde_json` depth limit in `legacy_preset.rs` | ❌ False positive | DoS-only (stack overflow crash); local file dialog; identical to existing findings #7/#8; excluded by DoS rule |
| `picked_color` in shared `EngineState` | ❌ False positive | Web server uses its own `WebServerState` — never serializes `EngineState`; no code path exposes `picked_color` to network clients |

### Impact on Existing Findings

| Finding | Status | Notes |
|---------|--------|-------|
| #1 Spout shared-memory dereference | Not worsened | Second input slot uses same existing `start_spout()` path; no new dereference logic |
| #2 Spout registry unsynchronized writes | Not worsened | Same existing registry code path |
| #3 D3D11 `.expect()` panics | Not worsened | No new D3D11 code |
| #4 Web remote auth bypass | Not worsened | PR doesn't touch `rustjay-control`; new waaaves params use same clamped parameter path already exposed |
| #5 macOS ObjC swizzling UB | Not worsened | No new swizzling code |
| #6 NDI thread leak | Not worsened | Second NDI slot uses same existing `start_ndi()` path |
| #7/#8 serde_json no depth limit | Not worsened | `legacy_preset.rs` adds a parallel instance of the same pattern, but impact is DoS-only |

### Priority Note

**Finding #4 (Web auth bypass) remains the only actionable security fix.** The Axum `route_layer` ordering flaw — where `/{app_name}` and `/` are registered *after* the middleware — is a concrete, trivially exploitable authentication bypass on any LAN. This PR's expanded waaaves parameter surface makes the exposed control surface marginally larger but does not change the fundamental flaw. Fix the route ordering and Origin validation before any network exposure.

---

## Executive Summary

The codebase is generally well-structured with good defensive patterns (clamping, atomic operations, bounded queues). However, **several high-severity security and stability issues require explicit attention**, primarily in the Windows Spout I/O path and the web remote control interface. The core engine and rendering pipeline are relatively safe, with risks concentrated in FFI/unsafe boundaries and external protocol handlers.

| Severity | Count | Categories |
|----------|-------|------------|
| 🔴 **Critical / High** | 6 | Spout memory safety, Web auth bypass, macOS UB, process aborts |
| 🟡 **Medium** | 8 | Thread leaks, panic paths, NaN poisoning, config DoS |
| 🟢 **Low / Info** | 12 | Style issues, unenforced invariants, missing validations |

---

## 🔴 CRITICAL / HIGH SEVERITY

### 1. Spout Input: Untrusted Shared-Memory Dereference Without Size Validation
**File:** `crates/rustjay-io/src/input/spout_input.rs` (`read_sender_dimensions`, `read_sender_info`)  
**Details:** The Spout input receiver maps shared memory from other processes and immediately dereferences it as `SharedTextureInfo` via raw pointers. A malicious process can create a spoofed sender with a mapped region smaller than `sizeof(SharedTextureInfo)`. This causes an out-of-bounds read and potential memory disclosure or crash.  
**Impact:** Information disclosure, denial of service, potential code execution if combined with other vulnerabilities.  
**Recommendation:** Validate the mapped region size is at least `std::mem::size_of::<SharedTextureInfo>()` before casting the pointer.

### 2. Spout Output: Unsynchronized Global Shared-Memory Registry Writes
**File:** `crates/rustjay-io/src/output/spout_output.rs` (`register_spout_sender`, `unregister_spout_sender`)  
**Details:** The Spout output sender modifies the global `SpoutSenderNames` shared-memory registry using raw pointer writes (`std::ptr::copy_nonoverlapping`, `std::ptr::write_bytes`) without any IPC synchronization primitive (named mutex, atomic, etc.). Multiple Spout senders (from other apps or other instances of this app) can race on this flat name array, corrupting the registry.  
**Impact:** Registry corruption, sender name collisions, potential use-after-free in consumer processes.  
**Recommendation:** Use a named mutex around registry modifications, or at minimum verify mapping size before raw pointer writes.

### 3. Spout Input: Process Abort on D3D11 Initialization Failure
**File:** `crates/rustjay-io/src/input/spout_input.rs` (lines 258, 262, 263)  
**Details:** `SpoutInputReceiver::new` calls `.expect()` on `D3D11CreateDevice`, `device`, and `context`. If the GPU driver is unavailable, Windows is in safe mode, or D3D11 is not supported, the entire process aborts instead of returning a recoverable error.  
**Impact:** Complete application crash on systems without D3D11.  
**Recommendation:** Replace all `.expect()` calls in Spout initialization with `anyhow::Result` propagation.

### 4. Web Remote Control: Authentication Bypass + Token Leak
**File:** `crates/rustjay-control/src/web/mod.rs` (`create_router`, `ws_handler`, `auth_middleware`)  
**Details:**
- The `.route_layer(auth_middleware)` is applied **before** the HTML page routes are added. Axum's `route_layer` only affects routes registered **prior** to it. The page route (`/{app_name}`) and root redirect (`/`) are added **after** the layer, so they bypass authentication entirely.
- The HTML page contains the bearer token injected via `inject_token_into_html`.
- An attacker on the same network can fetch the unprotected HTML page, extract `window.RUSTJAY_TOKEN`, and use it to authenticate WebSocket connections.
- The CORS layer allows `Any` origin, enabling cross-origin requests.
- The Origin header check in `ws_handler` is ineffective: it only rejects if Origin is present AND empty. Requests without an Origin header (e.g., from curl, Python scripts, or some proxies) pass through.

**Impact:** Complete bypass of web remote authentication. Any network-local attacker can read the token and control the engine remotely.  
**Recommendation:**
1. Move `.route_layer(auth_middleware)` to **after** all protected routes, or use `.layer(middleware::from_fn(...))` globally.
2. Strictly validate Origin against an allowlist.
3. Restrict CORS to same-origin only.
4. Consider serving the HTML via a separate path that also requires auth, or do not embed the token in HTML at all — instead require the user to enter it manually.

### 5. macOS App Delegate Swizzling: Undefined Behavior Risk
**File:** `crates/rustjay-engine/src/app/macos.rs` (line 46–77)  
**Details:** The code uses `unsafe` Objective-C runtime manipulation (`class_addMethod`, `std::mem::transmute`) to modify `WinitApplicationDelegate`'s internal methods at runtime. If winit's delegate class layout or method signatures change in a future version, this transmute will invoke UB (wrong function pointer type, stack corruption, or crashes).  
**Impact:** Potential undefined behavior, crash, or security vulnerability on winit upgrades.  
**Recommendation:** Document this as a fragile integration. Add compile-time checks for winit version, or monitor winit changelogs closely. Consider upstreaming the needed hooks to winit itself.

### 6. NDI Output Thread Intentionally Leaked
**File:** `crates/rustjay-io/src/output/ndi_output.rs` (line 63)  
**Details:** The NDI output worker thread is spawned but its `JoinHandle` is dropped without joining. On shutdown, if the thread is blocked in `recv_timeout`, it may persist as a zombie or hold resources.  
**Impact:** Resource leaks, unpredictable shutdown behavior, potential data loss on exit.  
**Recommendation:** Store the `JoinHandle` and call `join()` in `stop()` or `Drop`.

---

## 🟡 MEDIUM SEVERITY

### 7. Config File Deserialization Without Depth Limit
**File:** `crates/rustjay-engine/src/config.rs` (`AppSettings::load`)  
**Details:** A 1 MiB size limit is enforced, but `serde_json::from_str` does not limit recursion depth. A malicious JSON file with deeply nested structures could cause stack overflow during deserialization.  
**Impact:** Denial of service (stack overflow / crash).  
**Recommendation:** Use `serde_json::Deserializer::from_str` with a custom visitor that limits depth, or cap file size more aggressively.

### 8. Preset File Deserialization Without Depth Limit
**File:** `crates/rustjay-presets/src/presets.rs` (`Preset::load`)  
**Details:** Same issue as #7 — 1 MiB size limit but no recursion depth cap on `serde_json::from_str`.  
**Impact:** Denial of service.  
**Recommendation:** Same as #7.

### 9. EngineState Slice Length Mismatch Panic
**File:** `crates/rustjay-core/src/state.rs` (`reset_custom_params_to_base`)  
**Details:** Uses `copy_from_slice` between `custom_params` and `custom_param_bases`. These are public `Vec<f32>` fields with no invariant enforcement. If a bug or external code mutates one vector without the other, the engine panics.  
**Impact:** Denial of service (panic).  
**Recommendation:** Add a length check before `copy_from_slice`, or better, encapsulate the vectors behind an API that maintains the invariant.

### 10. `unreachable!()` In Readback Pool State Transition
**File:** `crates/rustjay-io/src/output/mod.rs` (line 110)  
**Details:** `ReadbackPool::harvest_previous` uses `unreachable!()` after a `std::mem::replace` that is expected to yield `SlotState::Pending`. If future refactoring violates this invariant, the code panics.  
**Impact:** Denial of service (panic) on code evolution.  
**Recommendation:** Replace `unreachable!()` with a graceful fallback (log + return None).

### 11. Plugin Renderer `unwrap()` on Mesh and Bind Group
**File:** `crates/rustjay-render/src/plugin_renderer.rs` (lines 500, 867)  
**Details:** Two `unwrap()` calls rely on imperative invariants: `mesh_vertex_buffer.as_ref().unwrap()` in `check_mesh_dirty` and `cached_pass_bind_groups[i].as_ref().unwrap()` in `render_graph`. The type system does not guard against future refactors that might violate these assumptions.  
**Impact:** Potential panic.  
**Recommendation:** Use `if let Some(...)` or return early with a log message.

### 12. Plugin Already Consumed Panic
**File:** `crates/rustjay-engine/src/app/events.rs` (line 54)  
**Details:** `self.plugin.take().expect("plugin already consumed")` will panic if `resumed()` is invoked twice. Not expected in normal winit flow, but a logic bug could trigger it.  
**Impact:** Denial of service (panic).  
**Recommendation:** Return early with a log error instead of panicking.

### 13. NaN / Inf Propagation in Audio Routing
**File:** `crates/rustjay-core/src/routing.rs` (`AudioRoute::process`)  
**Details:** Computes `(-delta_time / smoothing.max(0.001)).exp()`. If `delta_time` is negative (clock skew) or `smoothing` is NaN, the result can be `inf` or NaN, propagating into modulated parameters.  
**Impact:** Visual glitches, NaN poisoning downstream into shader uniforms.  
**Recommendation:** Clamp `delta_time` to `>= 0.0` and validate `smoothing` is finite.

### 14. NaN Propagation in LFO System
**File:** `crates/rustjay-core/src/lfo.rs` (`Lfo::calculate_value`, `update`)  
**Details:** `phase % 1.0` with NaN input yields NaN, which propagates through waveform functions into shader uniforms. The `update_parameter` in web server rejects NaN, but LFO output bypasses this.  
**Impact:** Visual glitches, NaN poisoning in GPU.  
**Recommendation:** Clamp or validate phase values before waveform calculation.

### 15. Syphon Discovery Blocks Main Thread
**File:** `crates/rustjay-io/src/input/mod.rs` (lines 311–316)  
**Details:** `begin_refresh_devices` runs `SyphonServerDirectory` synchronously on the caller's thread due to known AppKit/Metal deadlock issues with background threads. This blocks the main thread and can stall the UI for a noticeable period.  
**Impact:** UI freeze during device discovery.  
**Recommendation:** Documented trade-off. Consider adding a timeout or progress indicator.

---

## 🟢 LOW SEVERITY / STYLE & ROBUSTNESS

### 16. Silent Mutex Poison Recovery
**Files:** Throughout (`renderer.rs:138,247,346`, `commands.rs`, `events.rs`, all GUI tabs)  
**Details:** The codebase universally uses `mutex.lock().unwrap_or_else(|e| e.into_inner())` to recover from poisoned mutexes. This keeps the app alive but may leave `EngineState` in an inconsistent condition after a thread panic.  
**Impact:** Potential state corruption after a panic.  
**Recommendation:** This is a deliberate resilience trade-off. Consider logging a warning when poison is detected.

### 17. Pixel Upload Without Bounds Check
**File:** `crates/rustjay-render/src/texture.rs` (`InputTexture::update`)  
**Details:** The `data` slice length is not validated against `width * height * 4`. A caller providing incorrectly sized data will result in GPU memory corruption or visual artifacts.  
**Impact:** Visual corruption, potential GPU driver issues.  
**Recommendation:** Add an assertion or error return if `data.len() != (width * height * 4) as usize`.

### 18. V4L2 Path Parsing Without Validation
**File:** `crates/rustjay-engine/src/app/commands.rs` (line 139)  
**Details:** `StartV4l2` command parses `/dev/videoN` via string split and parse. The path is not validated against filesystem traversal. However, the parsed value is only used as an index to `start_webcam`.  
**Impact:** Minimal — no direct filesystem traversal vulnerability, but brittle parsing.  
**Recommendation:** Use `path.file_name()` and validate the stem with a regex.

### 19. NDI Source Matching Uses Substring
**File:** `crates/rustjay-io/src/input/ndi.rs` (line 99)  
**Details:** Source matching uses bidirectional `contains()` rather than exact equality. If two sources have overlapping names (e.g., "Camera" and "Camera 2"), the wrong source may be selected.  
**Impact:** Wrong input source selected.  
**Recommendation:** Use exact string equality or document the substring behavior clearly.

### 20. Build Script Panic on Missing Framework
**File:** `vendor/syphon-core/build.rs` (line 19–20)  
**Details:** Uses `.unwrap()` on `canonicalize()` and `.parent()`. If the Syphon framework is missing, the build aborts rather than gracefully disabling the feature.  
**Impact:** Build failure on systems without Syphon.  
**Recommendation:** Return a descriptive error or skip the build step.

### 21. Spout Output `unwrap()` on Shared Texture
**File:** `crates/rustjay-io/src/output/spout_output.rs` (line 155)  
**Details:** `self.shared_texture.as_ref().unwrap()` in `submit_frame`. If the texture is None (not initialized), this panics.  
**Impact:** Panic if submit is called before texture creation.  
**Recommendation:** Return an error or log and return early.

### 22. Synchronous GPU Readback Blocks Render Thread
**File:** `crates/rustjay-render/src/renderer.rs` (lines 324–347)  
**Details:** Pixel pick readback uses `map_async` + `poll(wait_indefinitely)`. If the GPU/driver hangs, the render thread blocks forever.  
**Impact:** Render thread deadlock on GPU failure.  
**Recommendation:** Use a timeout poll or make readback async across frames.

### 23. Custom Param Vectors Can Drift on Preset Load
**File:** `crates/rustjay-presets/src/presets.rs` (`apply_to_state`)  
**Details:** Restores custom params by ID matching. If descriptors have changed since the preset was saved, some values are silently skipped. No crash, but parameters may reset unexpectedly.  
**Impact:** User confusion, lost settings.  
**Recommendation:** Log warnings for unmatched preset parameters.

### 24. Wgpu Surface Format Fallback Assumes Array Non-Empty
**File:** `crates/rustjay-render/src/renderer.rs` (line 109), `crates/rustjay-gui/src/renderer.rs` (line 44)  
**Details:** `surface_caps.formats[0]` is used as fallback if Bgra8UnormSrgb/Bgra8Unorm are not found. The wgpu documentation guarantees at least one format, but this is an implicit assumption.  
**Impact:** Panic if wgpu returns an empty formats list (theoretical).  
**Recommendation:** Use `.first().ok_or(...)?` for explicit handling.

### 25. OSC Server: No Authentication
**File:** `crates/rustjay-control/src/osc/mod.rs`  
**Details:** OSC is a UDP protocol with no built-in authentication. If the user changes the bind host from `127.0.0.1` to `0.0.0.0`, anyone on the network can send OSC messages to mutate engine state. All numeric parameters are clamped, but booleans and enums can be toggled.  
**Impact:** Unauthorized control if bound to public interface.  
**Recommendation:** Document this clearly. Add a warning log when binding to non-loopback addresses.

---

## Logic & Control-Flow Correctness Assessment

### Correct Patterns (Positive Findings)
- **Command dispatching:** `dispatch_commands` in `commands.rs` uses `std::mem::replace` to atomically clear all command slots in a single lock acquisition. Clean and race-free.
- **Audio callback safety:** No allocations, no mutexes in the real-time audio callback. Uses only atomics (`AtomicU32`, `AtomicBool`).
- **Parameter clamping:** All externally influenced numeric state (HSB, audio params, custom params) is clamped before being written.
- **MIDI CC parsing:** Checks `message.len() >= 3` before indexing.
- **MTC decoding:** `msg_type` is masked to `0x07` before indexing into `[u8; 8]`.
- **Tap tempo bounds:** `avg_interval` is checked `> 0.1 && < 3.0` before BPM calculation.
- **FFT size validation:** Only 1024/2048/4096/8192 are accepted.
- **Mesh generation:** `cols.max(1)` and `rows.max(1)` prevent division by zero.
- **Web param diffing:** `update_parameter` uses `last_sent` cache to skip unchanged values, reducing mutex contention.
- **Web NaN guard:** Rejects non-finite values before broadcasting.

### Minor Logic Notes
- **if/else chains:** Generally exhaustive and well-formed. A few `#[allow(unreachable_patterns)]` attributes correctly handle feature-gated enum variants.
- **Early returns:** Used appropriately in real-time code (audio callback) and error paths.
- **Deadlock risk:** Low. The engine uses a single `std::sync::Mutex<EngineState>` as the central lock. The render thread drops the guard before re-locking for FPS updates, preventing self-deadlock. GUI tabs acquire short-lived locks. No nested lock ordering inversions were found.
- **Livelock risk:** None identified. No spin-wait loops. The OSC thread sleeps 1ms on `WouldBlock`.

---

## Recommendations Priority Matrix

| Priority | Issue | Effort |
|----------|-------|--------|
| **P0** | Fix Web auth bypass (#4) | Small |
| **P0** | Validate Spout shared-memory size (#1) | Small |
| **P0** | Add IPC sync to Spout registry (#2) | Medium |
| **P0** | Replace Spout `.expect()` with Result (#3) | Small |
| **P1** | Join NDI output thread on drop (#6) | Small |
| **P1** | Add depth limits to serde_json parsing (#7, #8) | Small |
| **P1** | Fix `reset_custom_params_to_base` panic (#9) | Small |
| **P1** | Replace `unreachable!()` in readback pool (#10) | Tiny |
| **P2** | Document macOS swizzling fragility (#5) | Tiny |
| **P2** | Add pixel upload bounds check (#17) | Tiny |
| **P2** | Sanitize V4L2 path parsing (#18) | Tiny |
| **P2** | Add timeout to GPU readback (#22) | Small |

---

## Files Not Personally Reviewed (Agent-Covered)

The following files were covered by automated agent exploration and grep analysis rather than line-by-line manual review:
- `crates/rustjay-io/src/input/ndi.rs`
- `crates/rustjay-io/src/input/spout_input.rs`
- `crates/rustjay-io/src/input/syphon_input.rs`
- `crates/rustjay-io/src/input/webcam.rs`
- `crates/rustjay-io/src/output/ndi_output.rs`
- `crates/rustjay-io/src/output/spout_output.rs`
- `crates/rustjay-io/src/output/syphon_output.rs`
- `crates/rustjay-io/src/output/v4l2_output.rs`
- `vendor/syphon-core/**/*.rs`
- `vendor/syphon-metal/**/*.rs`
- `vendor/syphon-wgpu/**/*.rs`
- `examples/**/*.rs`

All other ~50 source files were read in their entirety.

---

*Report compiled by manual source review + automated agent exploration + pattern grep analysis.*
