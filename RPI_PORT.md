# Raspberry Pi Port — Sputnik

Port the sputnik example to run on Raspberry Pi 3+ with a `--nogui` flag for single-output headless operation.

---

## Target hardware

| Pi model | GPU backend | Notes |
|----------|-------------|-------|
| Pi 4 / 5 | Vulkan (Mesa V3DV) | Primary target; best performance |
| Pi 3     | OpenGL ES (Mesa VC4) | Stretch goal; wgpu GL backend via EGL |

wgpu already selects the best available backend on Linux (`Backends::all()`) — no code change needed for backend selection.

---

## Task breakdown

### T01 — Add `nogui: bool` to the engine `App` struct

**Files:**
- `crates/rustjay-engine/src/app/mod.rs`
- `crates/rustjay-engine/src/lib.rs`

1. Add `pub(crate) nogui: bool` to the `App<P>` struct.
2. Update `App::new()` to accept `nogui: bool`. When true:
   - Force `state.output_fullscreen = true` before the window is created.
   - If `target_fps > 30`, cap it to 30 (conservative default for Pi).
3. Update `run_app()` signature to accept `nogui`.
4. Add two new public functions to `lib.rs`:

   ```rust
   pub fn run_headless<P: EffectPlugin>(plugin: P) -> Result<()>
   pub fn run_headless_with_tabs<P: EffectPlugin>(
       plugin: P,
       tabs: Vec<Box<dyn AnyGuiTab>>,
   ) -> Result<()>
   ```

5. Export both from `prelude`.

---

### T02 — Gate control window creation in `events.rs`

**File:** `crates/rustjay-engine/src/app/events.rs`

In `resumed()`, the control window + imgui block starts at the `if self.control_window.is_none()` guard (~line 92). Wrap the entire block:

```rust
if !self.nogui {
    // existing: create control window, ImGuiRenderer, ControlGui
}
```

All downstream references to `control_window`, `imgui_renderer`, and `control_gui` are already behind `if let Some(...)` patterns — they silently become no-ops when `None`.

---

### T03 — Parse `--nogui` in sputnik `main.rs`

**File:** `examples/sputnik/src/main.rs`

```rust
fn main() -> anyhow::Result<()> {
    // ... existing env_logger setup ...
    let nogui = std::env::args().any(|a| a == "--nogui");
    if nogui {
        rustjay_engine::run_headless_with_tabs(SputnikEffect, vec![Box::new(SputnikTab)])
    } else {
        rustjay_engine::run_with_tabs(SputnikEffect, vec![Box::new(SputnikTab)])
    }
}
```

No extra CLI parsing crate required.

---

### T04 — Pi build: disable NDI

NDI is unavailable on Pi. Build with:

```bash
cargo build --release --no-default-features --features webcam \
  --target aarch64-unknown-linux-gnu -p sputnik
```

No `Cargo.toml` change needed — the existing feature flags handle this. The `v4l` crate is already a Linux-only dep in `rustjay-render/Cargo.toml`.

---

### T05 — Cross-compilation setup

**New file:** `.cargo/config.toml`

```toml
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
```

**macOS prerequisites:**
```bash
brew install aarch64-linux-gnu-binutils
rustup target add aarch64-unknown-linux-gnu
```

Or use `cross` for a hermetic Docker build (recommended):
```bash
cargo install cross
cross build --release --no-default-features --features webcam \
  --target aarch64-unknown-linux-gnu -p sputnik
```

---

### T06 — Pi OS dependencies (Pi 4 / Bookworm)

```bash
sudo apt install \
  mesa-vulkan-drivers libvulkan1 vulkan-tools \
  libv4l-dev v4l-utils \
  libwayland-dev libxkbcommon-dev
```

Pi 3 needs no Vulkan packages — Mesa GL is the default and wgpu's GL backend handles it via EGL.

---

## Mesh resolution

Default mesh is 320×180 (~57 k vertices) — expensive on Pi 3. Users should save a preset with lower resolution (e.g. 160×90) via the Web UI after first boot, or control it via OSC at startup. No code change planned; document the trade-off instead.

---

## Control without the GUI

All three control paths remain fully functional in `--nogui` mode:

| Path | How to use |
|------|------------|
| OSC  | `oscsend <pi-ip> 9000 /rustjay/sputnik/displacement_scale f 0.5` |
| MIDI | USB MIDI controller plugged in — CCs map as normal |
| Web UI | `http://<pi-ip>:8080` in a browser on another device |

---

## Verification

```bash
# 1. macOS — existing behaviour unchanged
cargo run -p sputnik

# 2. macOS — headless smoke test
cargo run -p sputnik -- --nogui
# Expected: single fullscreen output window, no control window

# 3. Pi 4 (on-device)
RUST_LOG=info ./sputnik --nogui
# Expected: "wgpu: selected backend Vulkan" in log, fullscreen output

# 4. Control paths headless
#    Send OSC, connect MIDI, open Web UI — all should respond
```

---

## Out of scope

- Pi Camera Module (CSI/libcamera) — USB webcam via V4L2 is the target
- HDMI input capture — treated as a standard V4L2 device if used
- Systemd service / autostart — follow-on task
