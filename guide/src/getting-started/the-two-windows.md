# The Two Windows

Every rustjay-engine app opens two windows on startup.

## The Output window

The output window is your GPU canvas. It renders your effect full-resolution every frame, with no UI chrome — just pixels.

**Keyboard shortcuts** (focus on the output window):

| Key | Action |
|---|---|
| `Shift+F` | Toggle fullscreen |
| `Shift+T` | Tap tempo |
| `Shift+F1`–`Shift+F8` | Recall preset quick-slot |
| `Escape` | Quit |

When you go fullscreen, the control window stays open on whichever screen it's on — send it to your laptop display while the output window fills a projector.

## The Control window

The control window is an ImGui panel with a row of tabs. The built-in tabs are:

### Input tab

Select the video source for `@group(0) @binding(0)`:

- **Webcam** — any connected capture device; pick by name or index
- **NDI** — receive from any NDI source on the LAN (requires `ndi` feature)
- **Syphon** (macOS) — receive from any Syphon server (VDMX, Resolume, other effects)
- **Spout** (Windows) — same idea, DirectX-based
- **V4L2** (Linux) — virtual or physical video device

If no source is selected, the input texture is solid black.

### Audio tab

Shows the live audio input waveform and 8-band FFT spectrum, current BPM estimate, beat phase progress bar, and the Tempo & Sync section for [tempo source selection](../modulation/tempo-sync.md).

### LFO tab

Three LFO banks (A, B, C), each with:
- Waveform selector (Sine, Triangle, Saw, Square, Noise)
- Rate — Hz or beat division
- Depth — modulation amount
- Target — which declared parameter to modulate

See [LFOs](../modulation/lfo.md) for details.

### MIDI tab

Shows connected MIDI devices. Click **Learn** on any parameter to arm CC learn mode — move a knob on your controller to assign it. Shows the current CC mapping for each parameter.

### OSC tab

Displays the OSC server address and port (default `0.0.0.0:7770`). Parameters are addressable as `/rustjay/<param-id>`.

### Presets tab

Save and load snapshots of the full engine state + your effect state. Eight quick-slots correspond to `Shift+F1`–`Shift+F8` on the output window.

### Output tab

Configure where your rendered frames go beyond the output window:

- **NDI** — publish to the LAN as an NDI source
- **Syphon** / **Spout** — share with other VJ apps
- **V4L2** — write to a loopback device

### Sync tab (optional)

Only visible when the `link` or `prodj` feature is enabled. Lets you choose the active tempo source (Audio / Ableton Link / ProDJ Link) and shows the current Link session or ProDJ deck list.

## Custom tabs

Your effect can add its own tab to the control window, or replace a built-in tab. See [Custom Tabs](../building-ui/custom-tabs.md).
