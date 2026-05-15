# Security & Performance Audit Roadmap

**Date:** 2026-05-16  
**Scope:** `rustjay-engine` + `prodjlink-rs`  
**Auditors:** Security role (58–72/100), Performance role (52–72/100)

This document is the canonical briefing for agents and contributors fixing findings from the parallel security/performance audit. Each task is self-contained: file paths, line numbers, what to change, and acceptance criteria are all included. Work can be picked up independently — tasks within a phase do not depend on each other unless noted.

---

## Repository Map

| Repo | Path | Role |
|------|------|------|
| `rustjay-engine` | `/Users/ac/developer/rust/rustjay-engine` | Main VJ engine — Rust + wgpu + axum |
| `prodjlink-rs` | `/Users/ac/developer/rust/prodjlink-rs` | Pioneer ProDJ Link UDP/TCP protocol library |

The engine consumes `prodjlink-rs` via a git dep (`branch = "main"` — unpinned, see Task 1.1).

---

## Scores

| Codebase | Security | Performance |
|----------|----------|-------------|
| rustjay-engine | 72 / 100 | 72 / 100 |
| prodjlink-rs | 58 / 100 | 52 / 100 |

---

## Phase 1 — This Week (Critical, Low Risk)

### Task 1.1 — Pin `prodjlink-rs` git dependency
**Repo:** `rustjay-engine`  
**File:** `Cargo.toml` (workspace root)  
**Severity:** Security Critical (supply chain)  
**Effort:** 15 min

**Problem:** The engine references `prodjlink-rs` as `branch = "main"`. Any commit to that branch — including accidental or malicious ones — lands in the engine on the next `cargo update`.

**Fix:**
1. In `/Users/ac/developer/rust/prodjlink-rs`, run `git rev-parse HEAD` to get the current SHA.
2. In the engine's `Cargo.toml`, replace:
   ```toml
   prodjlink-rs = { git = "https://github.com/BlueJayLouche/prodjlink-rs", branch = "main" }
   ```
   with:
   ```toml
   prodjlink-rs = { git = "https://github.com/BlueJayLouche/prodjlink-rs", rev = "<sha>" }
   ```
3. Run `cargo check` to confirm nothing breaks.

**Acceptance:** `Cargo.toml` references a specific rev SHA, not a branch name.

---

### Task 1.2 — Strip unsafe characters from ProDJ Link strings at parse boundary ✅ DONE
**Repo:** `prodjlink-rs`  
**File:** `src/lib.rs`  
**Lines:** 592, 704–710, 1065–1075, 1383–1393  
**Severity:** Security Critical (DOM XSS compound vector)  
**Effort:** 1–2 hrs  
**Completed:** 2026-05-16 — `sanitize_text()` helper added above struct definitions; applied at all four decode sites (keep-alive name, CDJ status name, menu-packet UTF-16 title/artist, render-response UTF-16 title/artist). Strips control chars, bidi overrides, and HTML metacharacters (entity-encoded). Both repos pass `cargo check`.

**Problem:** `name`, `track_title`, and `track_artist` are decoded from attacker-controlled LAN packets and exposed as plain `String` with no sanitization. The engine's web UI renders these via `innerHTML` (engine audit H-1), so a malicious Pioneer-compatible device on the LAN can execute arbitrary JS in any connected browser.

**Fix:** Add a private `sanitize_text(s: &str) -> String` helper and call it on all three string fields at their decode sites. The helper should:
- Remove ASCII control characters (`\x00`–`\x1F`, `\x7F`)
- Remove Unicode bidi override characters: U+202A–U+202E, U+2066–U+2069, U+200F, U+061C
- Replace `<`, `>`, `&`, `"`, `'` with their HTML entities (`&lt;` etc.)
- Normalize to Unicode NFC

Example implementation:
```rust
fn sanitize_text(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(*c as u32,
            0x00..=0x1F | 0x7F |
            0x202A..=0x202E | 0x2066..=0x2069 |
            0x200F | 0x061C
        ))
        .map(|c| match c {
            '<'  => '＜',  // or use entity encoding if String → String
            '>'  => '＞',
            '&'  => '＆',
            '"'  => '＂',
            '\'' => '＇',
            c    => c,
        })
        .collect()
}
```
Alternatively use HTML entity strings:
```rust
fn sanitize_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c as u32 {
            0x00..=0x1F | 0x7F => {}
            0x202A..=0x202E | 0x2066..=0x2069 | 0x200F | 0x061C => {}
            _ => match c {
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '&' => out.push_str("&amp;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#x27;"),
                c => out.push(c),
            },
        }
    }
    out
}
```

Apply at:
- Line 592: `let name = sanitize_text(&String::from_utf8_lossy(...).trim());`
- Line 708–710: wrap the `name` construction
- Lines 1065–1075: wrap `title` and `artist` from menu packet decode
- Lines 1383–1393: wrap `title` and `artist` from render response decode

Also add a `# Safety / Trust` section to the rustdoc on `CdjDeviceInfo` and `TrackInfo` marking all string fields as attacker-controlled.

**Acceptance:** A device announcing `name = "<script>alert(1)</script>"` produces `&lt;script&gt;alert(1)&lt;/script&gt;` in the struct. Unit test this.

---

### Task 1.3 — Fix web UI DOM XSS (`innerHTML` → DOM API)
**Repo:** `rustjay-engine`  
**File:** `crates/rustjay-control/src/web/ui.html`  
**Lines:** 255–289  
**Severity:** Security High  
**Effort:** 1–2 hrs

**Problem:** `createControl()` interpolates `param.name`, `param.id`, and `param.options` into `innerHTML` and `oninput` attribute strings. Even after Task 1.2 sanitizes ProDJ strings, other sources of parameter names (plugins, presets loaded from disk) can still contain HTML.

**Fix:** Rewrite `createControl()` to use DOM construction instead of `innerHTML`:
```js
function createControl(param) {
    const div = document.createElement('div');
    div.className = 'control';

    const label = document.createElement('span');
    label.className = 'control-name';
    label.textContent = param.name;  // textContent, never innerHTML
    div.appendChild(label);

    if (param.type === 'float' || param.type === 'int') {
        const input = document.createElement('input');
        input.type = 'range';
        input.min = param.min ?? 0;
        input.max = param.max ?? 1;
        input.step = param.type === 'int' ? 1 : 0.001;
        input.value = param.value ?? 0;
        input.addEventListener('input', () => updateParam(param.id, input.value));
        div.appendChild(input);
    } else if (param.type === 'enum') {
        const select = document.createElement('select');
        (param.options ?? []).forEach(opt => {
            const option = document.createElement('option');
            option.textContent = opt;  // textContent
            option.value = opt;
            select.appendChild(option);
        });
        select.addEventListener('change', () => updateParam(param.id, select.value));
        div.appendChild(select);
    }
    return div;
}
```

Also add a `Content-Security-Policy` header in the axum handler that serves `ui.html`:
```rust
// In web/mod.rs, index_handler
Response::builder()
    .header("Content-Type", "text/html")
    .header("Content-Security-Policy",
        "default-src 'self'; script-src 'self' 'unsafe-inline'; \
         connect-src 'self' ws: wss: http: https:")
    .body(HTML_CONTENT.into())
```

**Acceptance:** Opening browser devtools → Console shows no XSS when a param with `name = "<img src=x onerror=alert(1)>"` is registered. The CSP header appears in the response.

---

### Task 1.4 — Bind web server and OSC to `127.0.0.1` by default; add bearer token
**Repo:** `rustjay-engine`  
**Files:**
- `crates/rustjay-control/src/web/mod.rs` lines 253–274, 358–369
- `crates/rustjay-control/src/osc/mod.rs` lines 292–294  
**Severity:** Security High  
**Effort:** 3–4 hrs

**Problem:** Both the HTTP/WebSocket control server and the OSC UDP listener bind to `0.0.0.0`, exposing control of every engine parameter to any host on the LAN with no authentication.

**Fix — Web server:**
1. Change default bind address from `([0,0,0,0], port)` to `([127,0,0,1], port)`.
2. Add a `--web-host` CLI flag (or config key) that allows opting into `0.0.0.0` explicitly.
3. Generate a per-launch bearer token on startup:
   ```rust
   use std::fmt::Write;
   fn generate_token() -> String {
       let bytes: [u8; 16] = rand::random(); // add `rand = "0.8"` to Cargo.toml
       bytes.iter().fold(String::new(), |mut s, b| { write!(s, "{:02x}", b).ok(); s })
   }
   ```
4. Print the token to stderr on startup: `eprintln!("Web control token: {token}");`
5. Add an axum middleware that checks `Authorization: Bearer <token>` on every request and rejects with 401 otherwise.
6. For the WebSocket upgrade, check the `Origin` header — reject if it does not match the server's own origin.
7. Replace `CorsLayer::permissive()` with:
   ```rust
   CorsLayer::new()
       .allow_origin(tower_http::cors::Any)  // or specific origin
       .allow_methods([Method::GET, Method::POST])
   ```

**Fix — OSC:**
1. Change `Ipv4Addr::UNSPECIFIED` to `Ipv4Addr::LOCALHOST` in `osc/mod.rs:292`.
2. Add a `--osc-host` config option for opt-in LAN binding.

**Acceptance:** Running `curl http://localhost:<port>/health` without the token returns 401. Running with `Authorization: Bearer <token>` returns 200. The OSC server no longer appears in `ss -lun` on the LAN interface.

---

### Task 1.5 — Precompute Hann window in audio stream builder
**Repo:** `rustjay-engine`  
**File:** `crates/rustjay-audio/src/fft.rs`  
**Lines:** 116–118 (window computation), build_stream functions  
**Severity:** Performance Critical  
**Effort:** 1–2 hrs

**Problem:** The Hann window is recomputed from scratch inside the real-time audio callback using 4096 `cos()` transcendental calls per invocation. The window shape is constant — it depends only on `fft_size` which never changes.

**Fix:**
1. Add a `hann_window: Vec<f32>` field to the audio stream struct (or pass it as a closure capture).
2. In `build_stream_f32` / `build_stream_i16` / `build_stream_u16`, precompute:
   ```rust
   let hann_window: Vec<f32> = (0..fft_size)
       .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / fft_size as f32).cos()))
       .collect();
   ```
3. In `process_audio_frame`, replace the per-sample `cos()` call with a multiply against the precomputed table:
   ```rust
   for ((s, w), w_out) in samples.iter().zip(hann_window.iter()).zip(windowed_buf.iter_mut()) {
       *w_out = s * w;
   }
   ```

**Acceptance:** Audio callback CPU time measurably reduced (profile with `cargo flamegraph` or check via `htop` during playback). No change in FFT output — the window values are identical.

---

### Task 1.6 — Add `cargo audit` and `cargo deny` to CI for both repos ✅ DONE
**Repos:** Both  
**Files:** `.github/workflows/ci.yml` in each repo  
**Severity:** Security Medium (supply chain hygiene)  
**Effort:** 30 min per repo  
**Completed:** 2026-05-16 — `cargo audit` step added to `prodjlink-rs/.github/workflows/ci.yml`; `.github/workflows/ci.yml` created for `rustjay-engine` (was missing entirely) with `cargo check --workspace`, `cargo clippy`, `cargo test`, and `cargo audit`.

**Fix:** Add to each CI workflow:
```yaml
- name: Security audit
  run: |
    cargo install cargo-audit --quiet
    cargo audit

- name: Dependency policy
  run: |
    cargo install cargo-deny --quiet
    cargo deny check advisories licenses
```

Create a minimal `deny.toml` at the repo root:
```toml
[advisories]
ignore = []  # add any intentional ignores with justification comment

[licenses]
allow = ["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib"]
```

**Acceptance:** CI fails if a known CVE advisory exists against any dependency.

---

## Phase 2 — Next Sprint (High Impact, Moderate Effort)

### Task 2.1 — Cap and bound config/preset deserialization
**Repo:** `rustjay-engine`  
**Files:**
- `crates/rustjay-presets/src/presets.rs` line 145
- `crates/rustjay-engine/src/config.rs` line 121  
**Severity:** Security Medium  
**Effort:** 2–3 hrs

**Problem:** `serde_json::from_str` is called with no file-size limit. Fields like `audio_fft_size: usize`, `internal_width: u32`, `output_width: u32` are not validated before being applied to engine state, allowing OOM or GPU texture exhaustion from a crafted preset file. `custom_values: HashMap<String, f32>` is unbounded.

**Fix:**
1. Before `serde_json::from_str`, read file size and reject files over 1 MiB:
   ```rust
   let metadata = std::fs::metadata(&path)?;
   if metadata.len() > 1_048_576 {
       return Err(anyhow::anyhow!("Preset file too large"));
   }
   ```
2. Add a `validate(&self) -> Result<()>` method on `Preset` and `AppSettings` that clamps/rejects out-of-range values:
   ```rust
   fn validate(&self) -> anyhow::Result<()> {
       const MAX_DIM: u32 = 4096;
       const VALID_FFT_SIZES: &[usize] = &[512, 1024, 2048, 4096, 8192];
       if self.internal_width > MAX_DIM || self.internal_height > MAX_DIM { ... }
       if !VALID_FFT_SIZES.contains(&self.audio_fft_size) { ... }
       if self.custom_values.len() > 256 { ... }
       Ok(())
   }
   ```
3. Call `validate()` immediately after `serde_json::from_str`.

**Acceptance:** Loading a crafted preset with `"output_width": 999999` returns an error rather than attempting a GPU texture allocation.

---

### Task 2.2 — Fix Spout shared-memory integer overflow
**Repo:** `rustjay-engine`  
**File:** `crates/rustjay-io/src/input/spout_input.rs`  
**Lines:** 430, 446–451  
**Severity:** Security Medium (Windows only)  
**Effort:** 1 hr

**Problem:** Dimensions read from `SharedTextureInfo` are used in `(w * h * 4) as usize` without checked arithmetic. For `w = h = 0xFFFF`, this is ~17 GB. `pixel_buffer.resize(needed, 0)` would attempt that allocation.

**Fix:**
```rust
// Replace:
let needed = (w * h * 4) as usize;
pixel_buffer.resize(needed, 0);

// With:
let needed = (w as usize)
    .checked_mul(h as usize)
    .and_then(|n| n.checked_mul(4))
    .filter(|&n| n <= 32 * 1024 * 1024) // reject > 32 MB (8K max)
    .ok_or_else(|| anyhow::anyhow!("Spout dimensions out of range: {}x{}", w, h))?;
pixel_buffer.resize(needed, 0);
```

Also clamp `w` and `h` to `16384` immediately after reading from `SharedTextureInfo`.

**Acceptance:** A forged `SharedTextureInfo` with `w = 0xFFFF, h = 0xFFFF` returns an error instead of attempting a 17 GB allocation.

---

### Task 2.3 — Clamp WASM webcam dimensions and parameter inputs
**Repo:** `rustjay-engine`  
**File:** `examples/webapp/src/lib.rs`  
**Lines:** 302–311 (webcam), exported `set_delay_*` functions  
**Severity:** Security Medium / Performance  
**Effort:** 30 min

**Fix:**
```rust
#[wasm_bindgen]
pub fn update_webcam_frame(data: &[u8], width: u32, height: u32) {
    const MAX_DIM: u32 = 4096;
    let width = width.min(MAX_DIM);
    let height = height.min(MAX_DIM);
    // ... rest of function
}

#[wasm_bindgen]
pub fn set_delay_r(v: i32) {
    let v = v.clamp(-64, 64);
    // ...
}
```

Also replace the `unwrap()` in the `request_animation_frame` closure with `if let Some(...)`.

**Acceptance:** Calling `update_webcam_frame(..., 65535, 65535)` from JS does not exhaust GPU memory.

---

### Task 2.4 — Cache multi-pass bind groups with generation tracking
**Repo:** `rustjay-engine`  
**File:** `crates/rustjay-render/src/plugin_renderer.rs`  
**Line:** 828  
**Severity:** Performance High  
**Effort:** 2–3 hrs

**Problem:** `device.create_bind_group()` is called on every frame inside `render_graph()` for multi-pass effects. The single-pass path already caches via `cached_texture_bind_group` / `cached_texture_gen`. ~180+ driver descriptor-set allocations/sec with a 3-pass effect.

**Fix:** Apply the same generation-keyed caching pattern used in the single-pass path:
1. Add a `cached_pass_bind_groups: Vec<Option<wgpu::BindGroup>>` and `cached_pass_texture_gens: Vec<u64>` to the graph render state.
2. Before creating the bind group for pass `i`, check if `cached_pass_texture_gens[i] == current texture_generation`.
3. Only call `device.create_bind_group(...)` when the generation has changed; otherwise reuse the cached bind group.

**Acceptance:** GPU frame time measurably reduced for effects with ≥2 passes. Profile before/after with `wgpu-profiler` or `renderdoc`.

---

### Task 2.5 — Replace `custom_params` HashMap clone with `Vec<f32>` indexed by descriptor position ✅ DONE
**Repo:** `rustjay-engine`  
**File:** `crates/rustjay-engine/src/app/update.rs`  
**Lines:** 133, 137, 157  
**Severity:** Performance High  
**Effort:** Half day

**Problem:** Every frame: `state.custom_params = state.custom_param_bases.clone()` allocates a new `HashMap<String, f32>`. Then `state.param_descriptors.clone()` is called twice (for `update_audio` and `update_lfo`) — each clone copies all `String` fields in `ParameterDescriptor`.

**Fix:**
1. Wrap `param_descriptors` in `Arc<Vec<ParameterDescriptor>>`. Cloning is now a pointer copy:
   ```rust
   // In EngineState
   param_descriptors: Arc<Vec<ParameterDescriptor>>,
   ```
2. Replace `custom_params: HashMap<String, f32>` with `custom_params: Vec<f32>` indexed by position in `param_descriptors`. Update all call sites that access by string key to use `param_descriptors.iter().position(|d| d.id == key)`.
3. The per-frame reset becomes `state.custom_params.iter_mut().zip(state.custom_param_bases.iter()).for_each(|(p, b)| *p = *b)` — no allocation, just a memcpy of floats.

**Acceptance:** No `HashMap::clone()` call in `update.rs` hot path. Verify with `cargo flamegraph` that `alloc` no longer appears in the `about_to_wait` frame.

---

### Task 2.6 — Introduce `ProDjSnapshot` to eliminate frame-path mutex contention
**Repo:** `prodjlink-rs`  
**File:** `src/lib.rs`  
**Severity:** Performance Critical (compounds engine's existing mutex problem)  
**Effort:** 1 day

**Problem:** The engine calls `cdj_devices()` + `current_track()` each frame, causing 10+ acquisitions of `ProDjLinkState`'s mutex per frame, contending with the listener thread at 5 Hz per CDJ.

**Fix:**
1. Define a lightweight snapshot struct:
   ```rust
   #[derive(Clone, Default)]
   pub struct ProDjSnapshot {
       pub devices: Vec<CdjDeviceInfo>,
       pub master_bpm: Option<f32>,
       pub current_track: Option<TrackInfo>,
   }
   ```
2. Add `snapshot: parking_lot::RwLock<Arc<ProDjSnapshot>>` to `ProDjLinkClient`.
3. In `parse_cdj_status` and `parse_render_response`, after writing to `ProDjLinkState`, update the snapshot only when state actually changed:
   ```rust
   *self.snapshot.write() = Arc::new(self.build_snapshot(&state));
   ```
4. Expose a `pub fn snapshot(&self) -> Arc<ProDjSnapshot>` that just clones the Arc pointer — no lock on the read path.
5. Update `prodj.rs` in the engine to call `client.snapshot()` once per frame instead of `cdj_devices()` + `current_track()`.

**Acceptance:** Frame-path lock acquisitions on `ProDjLinkState` drop to 0. The engine's `about_to_wait` no longer contends with the ProDJ listener thread.

---

### Task 2.7 — Replace unsafe `setsockopt` FFI with `socket2` in prodjlink-rs
**Repo:** `prodjlink-rs`  
**File:** `src/lib.rs`  
**Lines:** 386–396, 411–421  
**Severity:** Security Medium / code quality  
**Effort:** 30 min

**Problem:** Two `unsafe` FFI calls to `libc::setsockopt` — return values are silently discarded. `socket2 = "0.5"` is already a dependency and provides `set_reuse_port` safely.

**Fix:**
```rust
// Replace the unsafe blocks with:
use socket2::{Domain, Protocol, Socket, Type};
let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
socket.set_reuse_port(true)?;  // propagates error
socket.set_reuse_address(true)?;
// bind, convert to std socket...
let std_socket: std::net::UdpSocket = socket.into();
```

**Acceptance:** `grep -n "unsafe" src/lib.rs` returns 0 results. `cargo check` passes.

---

### Task 2.8 — Cap concurrent metadata-fetch threads and `pending_fetches`
**Repo:** `prodjlink-rs`  
**File:** `src/lib.rs`  
**Lines:** 1453 (thread spawn), 86 (`pending_fetches` field)  
**Severity:** Security High / Performance  
**Effort:** 1–2 hrs

**Problem:** Each unique `(device_id, slot, track_id)` from a spoofed UDP packet spawns an OS thread. `pending_fetches` has no size cap, so a burst of spoofed packets exhausts OS thread count.

**Fix:**
1. Add a `fetch_thread_count: Arc<AtomicUsize>` to track live fetch threads.
2. Before spawning, check and reject if `fetch_thread_count.load(Ordering::Relaxed) >= 4`.
3. Cap `pending_fetches.len()` at 32:
   ```rust
   if state.pending_fetches.len() >= 32 {
       return; // drop the fetch request
   }
   ```
4. Decrement `fetch_thread_count` at the end of each fetch thread (including on timeout/error paths).

**Acceptance:** Sending 100 spoofed track-change UDP packets results in at most 4 metadata-fetch threads spawned. `pending_fetches` never grows past 32.

---

### Task 2.9 — Replace front-removal `Vec` with `VecDeque` in prodjlink-rs
**Repo:** `prodjlink-rs`  
**File:** `src/lib.rs`  
**Lines:** 559 (`pending_metadata_packets.remove(0)`), 824 (`history.remove(0)`)  
**Severity:** Performance High  
**Effort:** 30 min

**Fix:**
```rust
// Change field types:
pending_metadata_packets: std::collections::VecDeque<PendingUdpPacket>,
track_history: std::collections::VecDeque<TrackChangeRecord>,

// Replace remove(0) with:
pending_metadata_packets.pop_front();
track_history.pop_front();

// Replace push(...) with:
pending_metadata_packets.push_back(packet);
track_history.push_back(record);
```

**Acceptance:** `cargo clippy` passes. `remove(0)` no longer appears in `src/lib.rs`.

---

### Task 2.10 — Dirty-flag on `parse_cdj_status` to stop per-packet String allocation
**Repo:** `prodjlink-rs`  
**File:** `src/lib.rs`  
**Lines:** 693–810  
**Severity:** Performance High  
**Effort:** 1–2 hrs

**Problem:** Every CDJ status packet (5 Hz per CDJ = 20 Hz with 4 decks) unconditionally replaces the device entry in the HashMap, allocating a new `String` for `name` even when nothing changed.

**Fix:**
1. Add `#[derive(PartialEq)]` to `DeckStatus` (all fields are `Copy`-compatible).
2. Before calling `state.devices.insert(...)`, check if the entry exists and is unchanged:
   ```rust
   if let Some(existing) = state.devices.get_mut(&device_id) {
       existing.last_seen = Instant::now();
       if existing.status != new_status {
           existing.status = new_status;
       }
       // only update name if bytes changed
       if existing.name != new_name {
           existing.name = new_name;
       }
   } else {
       state.devices.insert(device_id, CDJDevice { name: new_name, status: new_status, ... });
   }
   ```

**Acceptance:** In steady state (no changes from CDJs), `heaptrack` or `dhat` shows zero heap allocations in the `parse_cdj_status` path.

---

### Task 2.11 — Move workspace `[profile.release]` to root `Cargo.toml`
**Repo:** `rustjay-engine`  
**Files:** `Cargo.toml` (workspace root), `crates/rustjay-engine/Cargo.toml`  
**Severity:** Performance Low (but zero risk)  
**Effort:** 10 min

**Problem:** The optimal release profile (`lto = "fat"`, `codegen-units = 1`, `panic = "abort"`) is only in `crates/rustjay-engine/Cargo.toml`. Audio and sync crates compile with Cargo defaults (`lto = false`, `codegen-units = 16`), missing cross-crate inlining.

**Fix:** Move the `[profile.release]` block from `crates/rustjay-engine/Cargo.toml` to the workspace root `Cargo.toml`. Remove the duplicate from the crate-level file.

**Acceptance:** `cargo build --release` for all crates uses `lto = "fat"`. Confirm with `cargo build --release -v 2>&1 | grep lto`.

---

## Phase 3 — Medium-Term (Architectural Changes)

### Task 3.1 — Redesign `shared_state` access pattern (reduce 15–22 locks/frame to 2–3)
**Repo:** `rustjay-engine`  
**Files:** `crates/rustjay-engine/src/app/update.rs`, `events.rs`  
**Severity:** Performance Critical  
**Effort:** 1 week  
**Partial:** 2026-05-16 — Command-pop consolidation landed as a standalone PR: `dispatch_commands` now pops all 9 command fields in a single `lock()` call (was 9 separate locks), saving 8 mutex acquires per frame unconditionally. Each `process_*` method now accepts its command as a parameter. Full snapshot→apply redesign is the remaining work.

**Problem:** `about_to_wait` acquires `Arc<Mutex<EngineState>>` 15–22 times per frame across `update_audio`, `update_lfo`, `update_midi`, `update_osc`, `update_web`, `update_input`, the renderer, the FPS tracker, and `dispatch_commands`.

**Approach:**
1. At the start of `about_to_wait`, take a single read lock to snapshot all values needed by update functions, then release it.
2. Run all update logic against the snapshot (no lock held).
3. At the end of `about_to_wait`, take a single write lock to apply the computed changes back to `EngineState`.
4. Consider splitting `EngineState` into `Arc<ImmutableConfig>` (descriptors, mappings — read-only after init) and `Arc<Mutex<LiveState>>` (current param values, BPM, etc.).

**Acceptance:** Lock acquisition count per frame drops to ≤ 3, measurable via `parking_lot` instrumentation or a frame-time profiler.

---

### Task 3.2 — MTC decoder: replace `Mutex` with `AtomicU64` packed SMPTE ✅ DONE
**Repo:** `rustjay-engine`  
**File:** `crates/rustjay-control/src/midi/mtc.rs`  
**Lines:** 193–207  
**Severity:** Performance Critical + Security Medium (priority inversion)  
**Effort:** 1 day  
**Completed:** 2026-05-16 — `MtcRxState` and `Arc<Mutex<…>>` removed. `MtcPublished` added with `smpte: AtomicU64` (bits[4:0]=HH, [10:5]=MM, [16:11]=SS, [21:17]=FF, [23:22]=rate, [24]=running, [25]=playing), `last_qf_ms: AtomicU64`, and `source_device: Mutex<String>` (rarely changes). Each MIDI port closure owns its own `MtcDecoder` — no sharing, eliminating all 240 hot-path Mutex acquires/sec. `tick()` and `clone_state()` use atomic loads only. Exhaustive SMPTE round-trip unit test added (12.4M cases).

**Problem:** The MTC receive callback (on midir's real-time MIDI thread) acquires a `std::sync::Mutex` on every quarter-frame message — 240 times/sec at 30 fps MTC. Risk of priority inversion if the main thread holds the mutex during a slow operation.

**Fix:** Encode the four SMPTE fields (hours, minutes, seconds, frames) + frame rate as bit-packed `u64`, stored in an `AtomicU64` with `Ordering::Release`/`Ordering::Acquire`:
```rust
// Store: pack fields into u64
let packed = (hours as u64) | ((minutes as u64) << 8) | ((seconds as u64) << 16) | ((frames as u64) << 24) | ((fps_code as u64) << 32);
atomic_smpte.store(packed, Ordering::Release);

// Load: unpack
let packed = atomic_smpte.load(Ordering::Acquire);
let hours = (packed & 0xFF) as u8;
// etc.
```

**Acceptance:** `grep -n "Mutex" crates/rustjay-control/src/midi/mtc.rs` returns 0. MTC decode runs without acquiring any lock.

---

### Task 3.3 — Persist readback staging buffers across frames ✅ DONE
**Repo:** `rustjay-engine`  
**File:** `crates/rustjay-io/src/output/mod.rs`  
**Lines:** 142–146  
**Severity:** Performance High  
**Effort:** Half day  
**Completed:** 2026-05-16 — `SlotState::Available` now carries `Option<(wgpu::Buffer, u64)>`. On harvest, the unmapped buffer is stored back in the slot cache instead of being dropped. On `submit_copy`, the cached buffer is reused if the size matches; a new allocation only occurs on resolution change. Eliminates ~480 MB/sec GPU heap churn at 1080p/60fps NDI. Misleading "Drop it; a new one is cheap" comment removed.

**Problem:** A new `wgpu::Buffer` (8 MB for 1080p NDI) is allocated on every frame when NDI output is active — ~500 MB/s of GPU buffer churn.

**Fix:** Maintain a `Option<(wgpu::Buffer, u32, u32)>` (buffer, width, height) in the `ReadbackPool`. Reuse the buffer if dimensions are unchanged; only reallocate when resolution changes:
```rust
if self.staging.as_ref().map_or(true, |(_, w, h)| *w != width || *h != height) {
    self.staging = Some((device.create_buffer(&wgpu::BufferDescriptor { ... }), width, height));
}
let staging_buffer = &self.staging.as_ref().unwrap().0;
```

**Acceptance:** With NDI output active, `heaptrack` or `Instruments` shows no `wgpu::Buffer` allocations after the first frame.

---

### Task 3.4 — Diff-track web parameter broadcast ✅ DONE
**Repo:** `rustjay-engine`  
**File:** `crates/rustjay-engine/src/app/update.rs`  
**Lines:** 289–309  
**Severity:** Performance High  
**Effort:** 2–3 hrs

**Problem:** `server.update_parameter(...)` is called for every registered parameter every frame regardless of whether the value changed, causing continuous serde JSON serialization + tokio broadcast channel sends at 60–120 fps.

**Fix:** Maintain a `last_sent: HashMap<String, f32>` in the web server state. Only call `update_parameter` when `(new_value - last_sent).abs() > threshold` (e.g., `0.001` for floats):
```rust
if let Some(&last) = last_sent.get(&param_id) {
    if (new_value - last).abs() < 0.001 { continue; }
}
last_sent.insert(param_id.clone(), new_value);
server.update_parameter(param_id, new_value);
```

**Acceptance:** With no parameters changing, CPU usage from the web server path drops to near zero. With rapid parameter changes, updates still propagate within one frame.

**Edge cases fixed 2026-05-16:** `register_parameter` / `register_enum_parameter` now call `self.last_sent.remove(id)` so a re-registered param always sends its initial value (was silently skipped if the stale cached value matched). `update_parameter` now returns early on `!value.is_finite()` to prevent a NaN from looping as a continuous broadcast.

---

### Task 3.5 — Fix broadcast address calculation in prodjlink-rs ✅ DONE
**Repo:** `prodjlink-rs`  
**File:** `src/lib.rs`  
**Line:** 376  
**Severity:** Security Medium  
**Effort:** 1–2 hrs  
**Completed:** 2026-05-16 — Added `if-addrs = "0.13"` dependency. `compute_broadcast(local_ip)` uses `if_addrs::get_if_addrs()` to find the real netmask for the interface matching `local_ip`, computing broadcast as `ip | !mask`; falls back to /24 with a warning. Status socket now binds to `local_ip:50002` instead of `0.0.0.0:50002` to reject packets arriving on other interfaces (VPN, secondary NIC), with fallback to `0.0.0.0` if the specific-IP bind fails. Added `set_multicast_loop_v4(false)` on the announce socket.

**Problem:** `[local_ip[0], local_ip[1], local_ip[2], 255]` hard-codes `/24` assumption, incorrect on `/16`, `/25`, or other prefix lengths.

**Fix:** Use `libc::getifaddrs` (already a transitive dep) or add `if-addrs = "0.7"` to Cargo.toml to enumerate interfaces and find the matching netmask:
```rust
use if_addrs::get_if_addrs;
fn compute_broadcast(local_ip: Ipv4Addr) -> Ipv4Addr {
    for iface in get_if_addrs().unwrap_or_default() {
        if let if_addrs::IfAddr::V4(v4) = iface.addr {
            if v4.ip == local_ip {
                // broadcast = ip | ~mask
                let ip = u32::from(v4.ip);
                let mask = u32::from(v4.netmask);
                return Ipv4Addr::from(ip | !mask);
            }
        }
    }
    // fallback: /24
    Ipv4Addr::new(local_ip.octets()[0], local_ip.octets()[1], local_ip.octets()[2], 255)
}
```

**Acceptance:** On a `/16` network (e.g., `10.0.0.5/16`), the broadcast address is `10.0.255.255`, not `10.0.0.255`.

---

### Task 3.6 — Add cargo-fuzz targets for prodjlink-rs packet parsers ✅ DONE
**Repo:** `prodjlink-rs`  
**Severity:** Security Medium (long-term robustness)  
**Effort:** 2–3 hrs  
**Original completion:** 2026-05-16 (targets created).  
**Critical fix 2026-05-16:** The original fuzz targets called shadow re-implementations in `pub mod fuzz`, not the real parsers — meaning the fuzz corpus exercised no production code paths. Fixed by: (1) extracting `parse_render_response_from_bytes(&[u8])` from `parse_render_response` so the buffer-scanning logic is callable without a `TcpStream`; (2) rewriting all four fuzz stubs to call `ProDjLinkClient::parse_cdj_status`, `parse_mixer_on_air`, `parse_menu_packets`, and `parse_render_response_from_bytes` directly, constructing a minimal `ProDjLinkState` for state-bearing parsers.

**Problem:** No fuzz testing exists for the UDP/TCP packet parsers, which are the primary attack surface.

**Fix:**
1. Add a `fuzz` directory with `Cargo.toml` targeting `libfuzzer-sys`.
2. Create fuzz targets for:
   - `parse_cdj_status(&[u8])`
   - `parse_mixer_on_air(&[u8])`
   - `parse_render_response` (TCP stream simulation)
   - `parse_menu_packets` (UDP menu response)
3. Add a CI step that runs `cargo fuzz run <target> -- -max_total_time=60` on PRs touching the parser.

**Acceptance:** `cargo fuzz run fuzz_cdj_status` runs without panic for 60 seconds against random input.

---

## Phase 3 Additions — New Findings (2026-05-16)

New issues identified during the Phase 3 implementation audit. Not in the original roadmap.

### Finding A — `discover_dbserver_port` accepts attacker-controlled port ✅ DONE
**Repo:** `prodjlink-rs`  
**File:** `src/lib.rs` line 1037  
**Severity:** Security Medium (CWE-345)  
**Completed:** 2026-05-16 — Added range check: port < 1024 or > 49151 returns `None` with a warning. Prevents an attacker on the LAN from redirecting the metadata fetcher to an arbitrary port (e.g. a service on the CDJ host that accepts raw TCP writes).

---

### Finding B — TCP metadata fetch threads starvable by slowloris
**Repo:** `prodjlink-rs`  
**File:** `src/lib.rs` (fetch thread spawning, `MAX_FETCH_THREADS = 4`)  
**Severity:** Security Medium (DoS)  
**Effort:** 1–2 hrs

**Problem:** Four malicious peers can hold TCP connections open indefinitely (the per-call 5-second `set_read_timeout` resets on each partial read), filling all four fetch-thread slots and starving real CDJ metadata fetches.

**Fix:** Add a per-connection wall-clock deadline enforced inside `fetch_metadata` — if `start.elapsed() > Duration::from_secs(10)` break the read loop regardless of partial progress. Alternatively, move `fetch_metadata` to a `tokio` runtime and use `tokio::time::timeout`.

**Acceptance:** A peer that dribbles one byte every 4 seconds cannot hold a fetch thread for more than 10 seconds.

---

### Finding C — `std::sync::Mutex` poison silently continued in engine
**Repo:** `rustjay-engine`  
**File:** `crates/rustjay-engine/src/app/commands.rs` and `update.rs` (all `lock()` call sites)  
**Severity:** Security Low / Reliability Medium  
**Effort:** 30 min

**Problem:** Every `lock()` call uses `unwrap_or_else(|e| e.into_inner())`, silently resuming on a poisoned mutex with no log entry. A panic in any worker thread would leave `EngineState` in an inconsistent snapshot that the engine continues to use.

**Fix:** Add `log::error!("[Engine] Mutex poisoned — recovering: {:?}", e)` in each `into_inner()` call, or migrate `shared_state` to `parking_lot::Mutex` (no poisoning) as part of Task 3.1.

**Acceptance:** A panic in a worker thread produces a visible error log entry before resuming.

---

### Finding D — `get_pseudo_mac` leaks host IP in keep-alive MAC field
**Repo:** `prodjlink-rs`  
**File:** `src/lib.rs` line ~271  
**Severity:** Security Low (CWE-200, information disclosure)  
**Effort:** 15 min

**Problem:** `[0x02, 0xC0, 0x11, local_ip[1], local_ip[2], local_ip[3]]` encodes 24 bits of the host's LAN IP in the broadcasted keep-alive MAC address, visible to all hosts on the broadcast domain.

**Fix:** Generate a random MAC once at startup (seed with `rand::random::<[u8;3]>()`) and cache it. Persist across restarts if MAC stability matters for rekordbox device identity.

**Acceptance:** The keep-alive MAC field does not encode the host IP.

---

## Quick Reference: File × Task Matrix

### `rustjay-engine`

| File | Tasks |
|------|-------|
| `Cargo.toml` (workspace) | 1.1, 2.11 |
| `crates/rustjay-control/src/web/ui.html` | 1.3 |
| `crates/rustjay-control/src/web/mod.rs` | 1.4 |
| `crates/rustjay-control/src/osc/mod.rs` | 1.4 |
| `crates/rustjay-audio/src/fft.rs` | 1.5 |
| `crates/rustjay-presets/src/presets.rs` | 2.1 |
| `crates/rustjay-engine/src/config.rs` | 2.1 |
| `crates/rustjay-io/src/input/spout_input.rs` | 2.2 |
| `examples/webapp/src/lib.rs` | 2.3 |
| `crates/rustjay-render/src/plugin_renderer.rs` | 2.4 |
| `crates/rustjay-engine/src/app/update.rs` | 2.5, 3.4 |
| `crates/rustjay-engine/src/app/events.rs` | 3.1 |
| `crates/rustjay-control/src/midi/mtc.rs` | 3.2 |
| `crates/rustjay-io/src/output/mod.rs` | 3.3 |
| `.github/workflows/ci.yml` | 1.6 |

### `prodjlink-rs`

| File | Tasks |
|------|-------|
| `src/lib.rs` | 1.2, 2.7, 2.8, 2.9, 2.10, 3.5 |
| `.github/workflows/ci.yml` | 1.6 |
| `fuzz/` (new) | 3.6 |

---

## Briefing Template for Sub-Agents

When assigning a task to an agent, use this format:

```
You are working on [rustjay-engine | prodjlink-rs] at [path].

Your task is Task [N.N]: [Title] from AUDIT_ROADMAP.md.

Problem: [copy the Problem section]
Fix: [copy the Fix section]
Acceptance criteria: [copy the Acceptance section]

Do not modify files outside the listed paths.
Run `cargo check` and `cargo clippy` before reporting done.
```

---

## Notes for Reviewers

- Tasks 1.x are safe to merge independently — no inter-task dependencies.
- Task 2.5 (Vec<f32> params) must be completed before Task 3.1 (shared_state redesign) to avoid conflicts.
- Task 2.6 (ProDjSnapshot) in `prodjlink-rs` must land before Task 3.1 in the engine, since the engine's lock count reduction depends on reading from the snapshot.
- Task 1.2 (sanitize strings in prodjlink-rs) is a prerequisite for safely closing engine H-1 for ProDJ data paths.
- All tasks should be tested with `cargo test` + `cargo clippy -- -D warnings`. Performance tasks should include a brief before/after note in the PR description (flamegraph, htop snapshot, or similar).
