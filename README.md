# rustjay-engine

High-performance, cross-platform VJ engine built in Rust — for VJ applications what [Bevy](https://bevyengine.org/) is for games.

## What is it?

rustjay-engine is a Cargo workspace of focused crates that handles all the infrastructure of a live visual performance tool:

- GPU rendering via **wgpu** (Metal / Vulkan / DX12)
- Video I/O — **Webcam**, **NDI**, **Syphon** (macOS), **Spout** (Windows), **V4L2** (Linux)
- Real-time audio analysis — FFT, beat detection, 8-band spectrum
- Audio-reactive routing — FFT bands → any parameter
- **LFO** modulation (3 banks, 5 waveforms, tempo-sync)
- **MIDI** learn and mapping
- **OSC** server
- **Web** parameter server (REST + live push)
- **Presets** with quick-slots (Shift+F1–F8)
- Dual-window architecture — Control (ImGui) + Output (fullscreen GPU)

You bring the shader and the idea. The engine handles everything else.

## Workspace layout

```
rustjay-engine/
├── crates/
│   ├── rustjay-core        # EngineState, command enums, LFO, routing types
│   ├── rustjay-audio       # AudioAnalyzer, FFT, beat detection
│   ├── rustjay-io          # InputManager, OutputManager (Webcam/NDI/Syphon/Spout/V4L2)
│   ├── rustjay-control     # MIDI, OSC, Web server
│   ├── rustjay-presets     # Preset save/load/quick-slots
│   ├── rustjay-gui         # ImGui control window, all built-in tabs
│   ├── rustjay-render      # wgpu pipeline, blit, textures, uniforms
│   └── rustjay-engine      # Facade — app runner, config, re-exports
└── examples/
    └── template            # Reference app: HSB color + full I/O
```

## Quick start

```toml
# Cargo.toml
[dependencies]
rustjay-engine = { git = "https://github.com/BlueJayLouche/rustjay-engine" }
env_logger = "0.11"
log = "0.4"
anyhow = "1"
```

```rust
// src/main.rs
fn main() -> anyhow::Result<()> {
    env_logger::init();
    rustjay_engine::run("my-app")
}
```

That's it — you get a dual-window VJ app with the full built-in control panel.

## Running the template example

```sh
git clone https://github.com/BlueJayLouche/rustjay-engine
cd rustjay-engine
cargo run -p template
```

**Keyboard shortcuts** (output window):
- `Shift+F` — toggle fullscreen
- `Shift+T` — tap tempo
- `Shift+F1–F8` — recall preset quick-slots
- `Escape` — quit

## Platform requirements

| Platform | Notes |
|----------|-------|
| macOS    | Metal GPU. Syphon I/O via `syphon-rs`. Requires Xcode CLT. |
| Windows  | Vulkan or DX12. Spout I/O via DirectX interop. |
| Linux    | Vulkan. V4L2 loopback output. |

NDI requires the [NDI SDK](https://ndi.video/download-ndi-sdk/) installed and the `ndi` feature enabled (default on).

## Roadmap

- **Phase 1** ✅ Port rustjay-template into a structured workspace
- **Phase 2** `EffectPlugin` trait — bring your own shader and uniforms
- **Phase 3** Multi-input compositor, custom GUI tabs
- **Phase 4** Plugin hot-reload, example gallery

## License

MIT
