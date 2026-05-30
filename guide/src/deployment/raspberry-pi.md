# Raspberry Pi

rustjay-engine targets four Pi generations with different GPU paths:

| Model | GPU | wgpu backend | Vulkan |
|---|---|---|---|
| Pi 5 | VideoCore VII | Vulkan (V3DV) | ✓ |
| Pi 4 | VideoCore VI | Vulkan (V3DV) | ✓ |
| Pi 3 | VideoCore IV | OpenGL ES (EGL) | ✗ |
| Pi 2 | VideoCore IV | OpenGL ES (EGL) | ✗ |

Pi 4/5 use the Vulkan path. Pi 2/3 use the OpenGL ES backend via EGL — wgpu selects the best available backend automatically, but GLES must be compiled in (see below).

---

## Pi 4 / Pi 5 — Raspberry Pi OS Bookworm

### Runtime packages

```sh
sudo apt update && sudo apt install \
    mesa-vulkan-drivers libvulkan1 vulkan-tools \
    libv4l-dev v4l-utils \
    libasound2-dev \
    libwayland-dev libxkbcommon-dev
```

Verify Vulkan:

```sh
vulkaninfo --summary   # should show a V3DV device
```

### Cross-compiling from macOS / Linux

```sh
cross build --release --no-default-features --features webcam \
    --target aarch64-unknown-linux-gnu -p sputnik
```

---

## Pi 2 / Pi 3 — Arch Linux ARM (armv7)

This section documents deploying to a **Raspberry Pi 2 Model B** running **Arch Linux ARM** cross-compiled from **macOS Apple Silicon**. Most steps also apply to Pi 3.

### OS (Arch Linux ARM)

Install runtime libraries on the Pi:

```sh
sudo pacman -Sy alsa-lib v4l-utils libv4l
```

For a display server, install a minimal X11 stack:

```sh
sudo pacman -Sy xorg-server xorg-xinit xf86-video-fbdev \
    mesa mesa-utils libx11 libxext libxrandr libxinerama \
    libxcursor libxkbcommon-x11
```

Allow non-console users to run the X server:

```sh
sudo bash -c 'echo -e "allowed_users=anybody\nneeds_root_rights=yes" \
    > /etc/X11/Xwrapper.config'
```

For Wayland (needed for a reliable wgpu EGL display connection — see [Known issue](#known-issue-wgpu-egl-on-x11)):

```sh
sudo pacman -Sy weston seatd libdisplay-info
sudo systemctl enable --now seatd
sudo usermod -aG seat alarm
```

### Cross-compiling from Apple Silicon Mac

#### 1. Install cross from git

The published `cross 0.2.5` assumes an x86-64 Linux Docker host and tries to install `stable-x86_64-unknown-linux-gnu` on the macOS ARM host before Docker starts, which fails. Install the latest git HEAD:

```sh
cargo install --git https://github.com/cross-rs/cross cross --locked
```

#### 2. Cross.toml — Docker image + system libs

`Cross.toml` (workspace root) must specify the armv7 Docker image **and** install the ALSA/V4L/udev headers inside the container:

```toml
[target.armv7-unknown-linux-gnueabihf]
image = "ghcr.io/cross-rs/armv7-unknown-linux-gnueabihf:edge"
pre-build = [
    "dpkg --add-architecture armhf",
    "apt-get update && apt-get install -y libasound2-dev:armhf libv4l-dev:armhf libudev-dev:armhf",
]
```

#### 3. Workspace feature isolation

Cargo feature resolution is workspace-wide. Without explicit `default-features = false` at the **workspace definition level**, all workspace members (delta, waaaves, etc.) contribute their default features — including `ndi` — to every package's compilation even when building with `-p sputnik --no-default-features`.

The workspace `Cargo.toml` must have:

```toml
[workspace.dependencies]
rustjay-engine = { path = "crates/rustjay-engine", version = "0.1.0", default-features = false }
rustjay-io     = { path = "crates/rustjay-io",     version = "0.1.0", default-features = false }
```

> **Note:** setting `default-features = false` at the _package_ level (`{ workspace = true, default-features = false }`) does **not** override the workspace definition in Cargo 1.95. It must be set in `[workspace.dependencies]`.

Examples that need NDI/webcam must opt in explicitly via their own feature flags (delta, waaaves, etc.) or in their dep declaration (delta-egui: `features = ["egui", "ndi", "webcam"]`).

#### 4. wgpu GLES feature

The Pi 2 has no Vulkan. wgpu must be compiled with the `gles` feature so it can use Mesa's OpenGL ES via EGL:

```toml
# workspace Cargo.toml
wgpu = { version = "29.0", features = ["spirv", "gles"] }
```

#### 5. Build command

```sh
# sputnik (software rendering on Pi 2 — needs llvmpipe for compute shaders)
cross build --release --no-default-features --features webcam \
    --target armv7-unknown-linux-gnueabihf -p sputnik

# flux with DRM/KMS hardware path (no compositor required)
cross build --release --no-default-features --features webcam,drm-gles2 \
    --target armv7-unknown-linux-gnueabihf -p flux
```

The `drm-gles2` feature includes `gles2` and adds DRM/KMS + GBM surface support.
On Pi 4/5 you can omit these features — the standard wgpu Vulkan path is used instead.

#### 6. Deploy

Copy the binary to the Pi and restart the service:

```sh
scp target/armv7-unknown-linux-gnueabihf/release/flux alarm@<pi-ip>:/home/alarm/flux.new
ssh alarm@<pi-ip> '
    sudo systemctl stop flux
    sleep 1
    mv /home/alarm/flux.new /home/alarm/flux
    sudo systemctl start flux
'
```

### Running on Pi 2

Pi 2's VideoCore IV GPU supports **OpenGL ES 2.0** hardware. wgpu requires GLES 3.0 (specifically for Uniform Buffer Objects), so it cannot use the VC4 GPU directly. The two options are:

| Effect | Render path | How to run |
|---|---|---|
| **flux** | Native GLES 2.0 (VC4 hardware) | `./flux --nogui --gles2` |
| **sputnik** | llvmpipe software rendering | `LIBGL_ALWAYS_SOFTWARE=1 ./sputnik --nogui` |

**flux `--gles2 --drm`:** bypasses wgpu AND the Wayland compositor entirely. Opens `/dev/dri/card0` directly via KMS, creates a GBM surface, and renders using GLES 2.0 with GLSL ES 1.00 shaders on VC4 hardware. No weston, no X11, no `LIBGL_ALWAYS_SOFTWARE`.

**sputnik** uses compute shaders (mesh deformation) which VC4 does not support in hardware at any GLES version, so it remains on llvmpipe.

```sh
# Run flux directly on DRM — no compositor at all
RUST_LOG=warn ./flux --nogui --gles2 --drm

# Half display resolution (preserves aspect ratio, good default for Pi 2)
RUST_LOG=warn ./flux --nogui --gles2 --drm --render-scale 0.25

# Run sputnik — still requires software rendering (compute shaders)
XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-1 \
    LIBGL_ALWAYS_SOFTWARE=1 RUST_LOG=warn \
    ./sputnik --nogui
```

### Render resolution flags

By default flux renders at the full display resolution. On Pi 2 you almost always want to reduce this:

| Flag | Effect |
|---|---|
| `--render-scale 0.25` | Render at 25% of display dimensions (preserves aspect ratio) |
| `--render-scale 0.5` | Render at 50% of display dimensions |
| `--render-width W --render-height H` | Fixed render resolution (you are responsible for matching the display AR) |

`--render-scale` is preferred because it always matches the display's aspect ratio. Using a fixed `--render-width/--render-height` that differs from the display's aspect ratio will stretch the optical-flow feedback loop and change the visual character of the effect.

Typical values for Pi 2:
- HDMI output (16:9): `--render-scale 0.25` → 512×288 at 2048×1152
- Composite NTSC (4:3, 720×480): `--render-scale 0.5` → 360×240
- Composite PAL (4:3, 720×576): `--render-scale 0.5` → 360×288

> **Why does flux work but sputnik doesn't?** flux uses three plain fragment-shader passes — no UBOs visible to the GLES 2.0 path, no compute, no mesh. sputnik requires compute shaders (GLES 3.1 feature) that VC4 hardware never supports.

> **Pi 4 / Pi 5** support Vulkan natively. Use the standard `./flux --nogui` (no flags needed) and drop `LIBGL_ALWAYS_SOFTWARE=1` from sputnik as well.

> **DRM presentation on vc4:** `drmModePageFlip` returns `EBUSY` on Pi 2's VC4 driver regardless of flags. flux works around this by calling `drmModeSetCrtc` each frame. This is not vblank-locked but `eglSwapInterval(1)` is set so `eglSwapBuffers` gates on vsync, keeping tearing minimal.

---

## Running headless (`--nogui`)

Pass `--nogui` to suppress the control window and open the output fullscreen:

```sh
RUST_LOG=warn ./sputnik --nogui
```

When `--nogui` is active:

- Only the output window is created — no imgui control panel.
- The output opens **fullscreen** immediately.
- `target_fps` is capped at **30** (configurable via the Web UI after launch).
- Audio, MIDI, OSC, and the Web UI all remain fully functional.
- The last-used webcam (stored as `startup_webcam_device` in the app's config JSON) is **attached automatically** before the first frame renders.

## Webcam auto-attach

Any effect that uses a webcam input will re-attach the same webcam on the next `--nogui` launch without any user interaction.

**How it works:** when the engine shuts down it saves the active webcam's device index to `~/.config/rustjay/<app-name>.json` as `startup_webcam_device`. On the next launch the webcam is opened synchronously during engine initialisation, before the first frame is rendered.

**First-time setup:** run the effect once with the GUI, select the webcam from the Input tab, then quit. The index is written automatically. All subsequent `--nogui` launches will use it.

**Manual override:** edit the config JSON directly:

```json
{
  "startup_webcam_device": 0
}
```

A value of `0` opens `/dev/video0` (the first V4L2 capture device). Set to `null` to disable auto-attach.

> **Why synchronous?** On Pi 2 with software rendering (llvmpipe) a single 1080p frame can take 30+ seconds. The two-step `RefreshDevices → StartWebcam` queue that works on desktop would never dispatch before the first render completes. The engine therefore starts the webcam directly inside the initialisation path, before handing control to the render loop.

## Controlling without a GUI

| Path | How |
|---|---|
| **Web UI** | `http://<pi-ip>:8081/<app-name>` in any browser on the same network (e.g. `/flux`) |
| **OSC** | Send to `<pi-ip>:7770`, e.g. `/rustjay/sputnik/displacement_scale 0.5` |
| **MIDI** | USB MIDI controller — CCs map via MIDI Learn as normal |

Settings (MIDI mappings, last preset, FPS target) persist in `~/.config/rustjay/sputnik.json`.

### Web remote on headless Pi 2 / Pi 3

The Web UI starts automatically when the effect launches. On a headless embedded device you typically want **LAN trust mode** enabled so anyone on the same network can open the page without a bearer token.

Enable it in the app's config:

```json
{
  "web_host": "0.0.0.0",
  "web_port": 8081,
  "web_lan_trust": true
}
```

With `web_lan_trust: true`, opening `http://<pi-ip>:8081/flux` from a phone or laptop on the same network requires no password. The controls affect the shader in real time.

## Resource budgeting (Pi 2 / Pi 3)

All rendering on Pi 2/3 runs through llvmpipe on the CPU. The dominant cost is pixel count × pass count.

**Flux** — three full-screen fragment passes (flow, warp, blit). With `--gles2 --drm` the passes run on VC4 hardware. Use `--render-scale` to trade pixel count for framerate:

| `--render-scale` | Pixels (at 720×480 composite) | Typical Pi 2 fps |
|---|---|---|
| 1.0 (default) | 345 600 | ~15 fps |
| 0.5 | 86 400 | ~30 fps |
| 0.25 | 21 600 | ~60 fps |

The optical-flow webcam capture always runs at 640×480 regardless of render scale. `--render-scale` only controls the internal FBO size for the warp and accumulation passes.

**Sputnik** — Dial back mesh resolution via the Web UI or a preset:

| Mesh resolution | Vertices | Suitable for |
|---|---|---|
| 320 × 180 | ~57 k | Pi 5 / Pi 4 |
| 160 × 90  | ~14 k | Pi 4, Pi 3 manageable |
| 80 × 45   | ~3.5 k | Pi 2 / Pi 3 safe starting point |

Use **Web UI → Sputnik tab → Mesh Resolution** to change at runtime, then save as a preset.

## Autostart on boot (Arch Linux ARM)

### 1. Add the user to the required groups

```sh
sudo usermod -aG video,audio alarm
# reboot for the change to take effect
```

`video` grants access to `/dev/dri/card0` and `/dev/video0`. `audio` grants ALSA sequencer access for MIDI.

### 2. Create the flux service (Pi 2 — DRM/KMS, no compositor)

`ExecStartPre=/bin/sleep 3` gives the kernel time to enumerate USB devices before flux opens `/dev/video0`.

```ini
# /etc/systemd/system/flux.service
[Unit]
Description=Flux VJ effect (optical-flow webcam warp — DRM/KMS direct)
After=multi-user.target
Wants=dev-video0.device

[Service]
User=alarm
Environment=RUST_LOG=warn
ExecStartPre=/bin/sleep 3
ExecStart=/home/alarm/flux --nogui --gles2 --drm --render-scale 0.25
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

No weston service is needed. flux opens `/dev/dri/card0` directly.

For `sputnik.service` on Pi 2, weston is still required (llvmpipe path). Use the Wayland-based setup from a previous section and add `Environment=LIBGL_ALWAYS_SOFTWARE=1`.

### 3. Enable

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now flux
journalctl -u flux -f
```

Verify on the next boot:

```sh
systemctl is-active flux        # should print "active"
fuser /dev/dri/card0            # should show the flux PID
fuser /dev/video0               # same PID — webcam open
ps aux | grep weston            # should be empty
```

> **Pi 4/5:** The `--gles2 --drm` flags are not needed — use the standard wgpu Vulkan path (`./flux --nogui`) and a normal Wayland or fullscreen setup.

## SD card protection (read-only root)

Unexpected power cuts can corrupt the SD card. Keep the root filesystem read-only during normal operation and remount read-write only when you need to deploy updates or edit configs.

### 1. Configure journald to use RAM

Stop systemd-journald from writing logs to disk:

```sh
sudo mkdir -p /etc/systemd/journald.conf.d
cat << 'EOF' | sudo tee /etc/systemd/journald.conf.d/volatile.conf
[Journal]
Storage=volatile
EOF
```

### 2. Create ro / rw scripts

`/usr/local/bin/ro` — stop writers, sync, remount read-only:

```sh
sudo tee /usr/local/bin/ro << 'EOF'
#!/bin/bash
set -e
sudo systemctl stop flux 2>/dev/null || true
sudo systemctl stop systemd-timesyncd 2>/dev/null || true
sudo systemctl stop systemd-journald 2>/dev/null || true
sudo sync
sudo mount -o remount,ro /
echo "Root filesystem is now READ-ONLY"
EOF
sudo chmod +x /usr/local/bin/ro
```

`/usr/local/bin/rw` — remount read-write:

```sh
sudo tee /usr/local/bin/rw << 'EOF'
#!/bin/bash
set -e
sudo mount -o remount,rw /
echo "Root filesystem is now READ-WRITE"
EOF
sudo chmod +x /usr/local/bin/rw
```

### 3. Passwordless sudo

Allow the `alarm` user to run the scripts without a password:

```sh
echo "alarm ALL=(ALL) NOPASSWD: /usr/local/bin/ro, /usr/local/bin/rw, /bin/mount" \
    | sudo tee /etc/sudoers.d/alarm-ro-rw
sudo chmod 440 /etc/sudoers.d/alarm-ro-rw
```

### Workflow

```sh
# Normal state — SD card is protected
ro

# Deploy a new build — remount rw, copy binary, then go back to ro
rw
scp target/armv7-unknown-linux-gnueabihf/release/flux alarm@pi:/home/alarm/flux.new
sudo systemctl stop flux
mv /home/alarm/flux.new /home/alarm/flux
sudo systemctl start flux
ro
```

> **Before unplugging the power:** run `ro` to ensure all writes are flushed and the filesystem is clean.
