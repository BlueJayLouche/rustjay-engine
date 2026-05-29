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
cross build --release --no-default-features --features webcam \
    --target armv7-unknown-linux-gnueabihf -p sputnik
```

#### 6. Deploy

```sh
scp target/armv7-unknown-linux-gnueabihf/release/sputnik alarm@<pi-ip>:~/
```

### Running on Pi 2

Pi 2's VideoCore IV GPU supports a maximum of **OpenGL ES 2.0** (hardware). wgpu requires GLES 3.0+ and the mesh deformation pass needs compute shaders (added in GLES 3.1). Hardware GPU rendering is therefore not viable on Pi 2.

**Use Mesa's software renderer (llvmpipe)** instead. llvmpipe exposes OpenGL 4.6 / GLES 3.2 in software. Performance is CPU-bound (~20% CPU at 30 fps with reduced mesh resolution), but it runs correctly.

Start weston and sputnik:

```sh
# Ensure seatd is running
sudo systemctl start seatd

# Start weston DRM compositor
XDG_RUNTIME_DIR=/run/user/1000 weston --backend=drm --no-config --idle-time=0 &
sleep 3

# Run sputnik with software rendering
XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-1 \
    LIBGL_ALWAYS_SOFTWARE=1 RUST_LOG=warn \
    ./sputnik --nogui
```

> **Why Wayland and not X11?** wgpu 29.0 initialises EGL before creating a window, and the default EGL fallback (surfaceless) cannot create windowed surfaces. Wayland provides the display handle at startup so EGL uses the correct platform. X11 requires additional engine changes that are not yet merged (see wgpu-hal issue with duplicate `SURFACE_TYPE` attributes on vc4 configs).

> **Why software rendering?** vc4 GLES 2.0 hardware lacks compute shaders (GLES 3.1 feature required for mesh deformation). `MESA_GLES_VERSION_OVERRIDE=3.0` tricks the API version string but does not add compute support. `LIBGL_ALWAYS_SOFTWARE=1` forces Mesa's llvmpipe which fully supports compute.

> **Pi 4 / Pi 5** do not need software rendering — VideoCore VI/VII supports GLES 3.1 / Vulkan natively and the hardware path works without any overrides.

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

## Controlling without a GUI

| Path | How |
|---|---|
| **Web UI** | `http://<pi-ip>:8081` in any browser on the same network |
| **OSC** | Send to `<pi-ip>:7770`, e.g. `/rustjay/sputnik/displacement_scale 0.5` |
| **MIDI** | USB MIDI controller — CCs map via MIDI Learn as normal |

Settings (MIDI mappings, last preset, FPS target) persist in `~/.config/rustjay/sputnik.json`.

## Resource budgeting (Pi 2 / Pi 3)

The VideoCore IV GPU is much weaker than a desktop GPU. Dial back mesh resolution via the Web UI or a preset:

| Resolution | Vertices | Suitable for |
|---|---|---|
| 320 × 180 | ~57 k | Pi 5 / Pi 4 |
| 160 × 90 | ~14 k | Pi 4, Pi 3 manageable |
| 80 × 45 | ~3.5 k | Pi 2 / Pi 3 safe starting point |

Use **Web UI → Sputnik tab → Mesh Resolution** to change at runtime, then save as a preset.

## Autostart on boot (Arch Linux ARM + Wayland)

```ini
# /etc/systemd/system/sputnik.service
[Unit]
Description=Sputnik VJ effect
After=weston.service

[Service]
User=alarm
Environment=WAYLAND_DISPLAY=wayland-1
Environment=XDG_RUNTIME_DIR=/run/user/1000
Environment=RUST_LOG=warn
ExecStart=/home/alarm/sputnik --nogui
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl enable --now sputnik
journalctl -u sputnik -f
```
