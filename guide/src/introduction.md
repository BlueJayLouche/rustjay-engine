# Introduction

**rustjay-engine** is a Rust framework for building real-time VJ (video jockey) applications. It handles all the infrastructure of a live visual performance tool — GPU rendering, video I/O, audio analysis, parameter modulation, and network control — so you can focus on the one thing that makes your effect unique: the shader.

> You bring the shader and the idea. The engine handles everything else.

## What you get out of the box

When you implement the `EffectPlugin` trait and call `rustjay_engine::run()`, you get a complete dual-window application:

- **Output window** — fullscreen GPU-rendered output powered by wgpu (Metal / Vulkan / DX12)
- **Control window** — a tabbed ImGui panel with built-in sections for input sources, audio analysis, LFO modulation, MIDI mapping, output routing, and presets

No threading setup. No window management. No audio callback plumbing. No preset serialisation. Those are solved problems.

## Key features

| Category | What's included |
|---|---|
| **Rendering** | wgpu (Metal / Vulkan / DX12), single-pass, multi-pass, frame-history ring buffers, indexed mesh displacement |
| **Video input** | Webcam, NDI, Syphon (macOS), Spout (Windows), V4L2 (Linux) |
| **Video output** | Window, NDI, Syphon, Spout, V4L2 |
| **Audio** | 8-band FFT, beat detection, BPM estimation, tap tempo |
| **Modulation** | 3 LFO banks × 5 waveforms, tempo-sync, audio-to-param routing |
| **Tempo sync** | Ableton Link, Pioneer ProDJ Link, MIDI Timecode (optional features) |
| **Control** | MIDI learn, OSC server, REST + WebSocket web remote |
| **Presets** | Full state snapshots, 8 quick-slots (Shift+F1–F8) |

## Who this is for

rustjay-engine is aimed at:

- **VJs and live visual artists** who want to write their own WGSL shaders without rebuilding all the surrounding infrastructure every time
- **Creative coders** with some Rust experience who want a structured framework for real-time GPU work
- **Developers** building tools for live performance (AV apps, installations, generative visuals)

This guide assumes you're comfortable with basic Rust — you know what traits are, can read a `Cargo.toml`, and have used `cargo build` before. You don't need deep GPU or wgpu knowledge to write your first effect; that becomes relevant when you move into [custom render pipelines](rendering/frame-history.md) later.

## How to use this guide

Start with [Installation](installation.md), then work through [Your First Effect](getting-started/README.md). After that, the chapters are largely independent — jump to whatever is relevant to what you're building.

The best way to learn is to run the examples alongside the guide:

```sh
git clone https://github.com/BlueJayLouche/rustjay-engine
cd rustjay-engine
cargo run -p template    # HSB colour — the simplest effect
cargo run -p delta       # RGB delay / motion extraction
cargo run -p waaaves     # multi-pass feedback pipeline
cargo run -p sputnik     # mesh displacement (Rutt-Etra style)
```
