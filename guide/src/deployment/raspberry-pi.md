# Raspberry Pi

rustjay-engine runs on Raspberry Pi 4 and Pi 5 via the Vulkan backend (Mesa V3DV). Pi 3 is also supported using the OpenGL ES backend (Mesa VC4) — wgpu selects the best available backend automatically.

> **Pi 3 note** — Pi 3 has no Vulkan driver. wgpu falls back to its OpenGL ES backend via EGL. The sputnik default mesh (320×180, 57 k vertices) is heavy for the VideoCore IV GPU; start with a lower resolution (see [Resource budgeting](#resource-budgeting)).

## OS prerequisites

These instructions target **Raspberry Pi OS Bookworm (64-bit)**.

### Pi 4 / Pi 5 — Vulkan

```sh
sudo apt update
sudo apt install \
    mesa-vulkan-drivers libvulkan1 vulkan-tools \
    libv4l-dev v4l-utils \
    libwayland-dev libxkbcommon-dev \
    build-essential pkg-config
```

Verify Vulkan is working:

```sh
vulkaninfo --summary
```

You should see a `V3DV` device listed.

### Pi 3 — OpenGL ES

No Vulkan packages are needed. Mesa GL is installed by default. Install only the V4L2 and build headers:

```sh
sudo apt update
sudo apt install \
    libv4l-dev v4l-utils \
    libwayland-dev libxkbcommon-dev \
    build-essential pkg-config
```

## Building for Pi

### On the Pi itself

Install Rust:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then build sputnik with NDI disabled (NDI SDK is not available on Pi):

```sh
cargo build --release --no-default-features --features webcam -p sputnik
```

The binary lands at `target/release/sputnik`.

### Cross-compiling from macOS or Linux

The workspace ships a `.cargo/config.toml` that sets the `aarch64-unknown-linux-gnu` linker. Install `cross` for a hermetic Docker build:

```sh
cargo install cross
rustup target add aarch64-unknown-linux-gnu
```

Build:

```sh
cross build --release --no-default-features --features webcam \
    --target aarch64-unknown-linux-gnu -p sputnik
```

Copy the result to the Pi:

```sh
scp target/aarch64-unknown-linux-gnu/release/sputnik pi@<pi-ip>:~/sputnik
```

## Running headless

Pass `--nogui` to suppress the control window and open the output fullscreen. This is the normal mode for a Pi with a single HDMI output:

```sh
RUST_LOG=info ./sputnik --nogui
```

When `--nogui` is active:

- Only the output window is created — no imgui control panel.
- The output opens **fullscreen** immediately.
- `target_fps` is capped at **30** if not already lower (configurable via the Web UI after launch).
- Audio, MIDI, OSC, and the Web UI all remain fully functional as control paths.

## Controlling the effect without a GUI

With `--nogui`, use any combination of:

| Path | How |
|---|---|
| **Web UI** | Open `http://<pi-ip>:3000` in a browser on any device on the same network |
| **OSC** | Send to `<pi-ip>:7770`, e.g. `/rustjay/sputnik/displacement_scale 0.5` |
| **MIDI** | Plug in a USB MIDI controller — CCs map via MIDI Learn as normal |

Settings (MIDI mappings, last preset, FPS target) are saved to `~/.config/rustjay/sputnik.json` and restored on next launch.

## Resource budgeting

The Pi's GPU is significantly weaker than a desktop. A few knobs to pull:

### Mesh resolution

The default 320×180 mesh (57 k vertices) is designed for desktop GPUs. Dial it back via the Web UI or by saving a preset:

| Resolution | Vertices | Suitable for |
|---|---|---|
| 320 × 180 | ~57 k | Pi 5 / Pi 4 (headroom to spare) |
| 160 × 90 | ~14 k | Pi 4 comfortable, Pi 3 manageable |
| 80 × 45 | ~3.5 k | Pi 3 safe starting point |

Use the **Web UI → Sputnik tab → Mesh Resolution** fields to change resolution at runtime, then save as a preset.

### Frame rate

The engine defaults to 60 fps; `--nogui` caps this to 30. To set a custom target at runtime, use the Web UI **Settings** panel or send an OSC message:

```sh
# Cap to 25 fps (PAL)
oscsend <pi-ip> 7770 /rustjay/target_fps i 25
```

The cap is also saved in `~/.config/rustjay/sputnik.json`.

### Webcam resolution

Request a lower capture resolution from your webcam by setting the input size in the Web UI (**I/O → Input → Resolution**). 640×360 gives the compute shader half the texture samples to fetch per vertex vs. 1280×720, with a modest visual difference for the Rutt-Etra effect.

## Autostart on boot

Create a systemd service to launch sputnik at boot:

```ini
# /etc/systemd/system/sputnik.service
[Unit]
Description=Sputnik VJ effect
After=graphical.target

[Service]
User=pi
Environment=DISPLAY=:0
Environment=WAYLAND_DISPLAY=wayland-0
Environment=XDG_RUNTIME_DIR=/run/user/1000
Environment=RUST_LOG=warn
ExecStart=/home/pi/sputnik --nogui
Restart=on-failure
RestartSec=5

[Install]
WantedBy=graphical.target
```

Enable it:

```sh
sudo systemctl enable sputnik
sudo systemctl start sputnik
```

Logs:

```sh
journalctl -u sputnik -f
```
