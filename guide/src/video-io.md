# Video I/O

rustjay-engine supports multiple video input and output protocols. All device discovery runs in a background thread — the GPU render loop never blocks waiting for a camera or network source.

## Video inputs

Configure the input source from the **Input** tab at runtime. Your shader always receives the current frame at `@group(0) @binding(0)` regardless of which source is active.

### Webcam

Any V4L2 (Linux), AVFoundation (macOS), or DirectShow (Windows) capture device. The Input tab lists available devices by name; select one and the engine opens it at its native resolution.

No feature flag needed.

### NDI

Network Device Interface — receive video from any NDI source on the LAN (OBS, Resolume, another rustjay-engine instance).

Requires the [NDI SDK](https://ndi.video/download-ndi-sdk/) and the `ndi` feature (on by default).

The Input tab scans the LAN and shows available sources. Low latency, high resolution, suitable for multi-machine setups.

### Syphon (macOS only)

Receive frames from any Syphon server on the same machine — VDMX, Resolume, Final Cut Pro, another rustjay-engine app.

Requires Syphon.framework in `/Library/Frameworks/`. No feature flag (always compiled on macOS).

Zero-copy GPU texture sharing — no CPU round-trip.

### Spout (Windows only)

The Windows equivalent of Syphon. Receive frames from Resolume, MadMapper, or any Spout-compatible app.

Uses DirectX texture sharing. No feature flag (always compiled on Windows).

### V4L2 (Linux only)

Receive from any `/dev/videoN` device, including loopback devices created by the `v4l2loopback` kernel module. Can receive video piped from OBS or ffmpeg.

## Video outputs

Configure from the **Output** tab.

### NDI output

Publish the rendered output as an NDI source on the LAN. Other NDI receivers see it immediately.

```toml
rustjay-engine = { git = "...", features = ["ndi"] }
```

### Syphon output (macOS)

Publish as a Syphon server. VDMX, Resolume, and other VJ apps on the same machine can receive and further process the output.

### Spout output (Windows)

Publish as a Spout sender for DirectX-based apps on the same machine.

### V4L2 output (Linux)

Write frames to a V4L2 loopback device (`/dev/videoN`) so tools like OBS, ffmpeg, or Zoom can receive the output as a virtual camera.

```sh
# Create a loopback device first
sudo modprobe v4l2loopback
```

## Resolution and frame rate

The output window renders at the resolution it was created at (typically display DPI-scaled). Output protocols (NDI, Syphon, Spout) capture the rendered frame at that resolution.

Frame rate is uncapped by default and limited by GPU rendering time. Set a target rate in the engine config if you need to control pacing.

## No input

If no source is selected, the engine provides a solid-black texture at group 0 binding 0. Your shader still runs — useful for generative effects that don't need video input.
