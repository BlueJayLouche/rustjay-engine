# rustjay-engine

High-performance, cross-platform VJ engine built in Rust — for VJ applications what [Bevy](https://bevyengine.org/) is for games.

## What is it?

rustjay-engine is a Cargo workspace of focused crates that handles all the infrastructure of a live visual performance tool:

- GPU rendering via **wgpu** (Metal / Vulkan / DX12)
- Video I/O — **Webcam**, **NDI**, **Syphon** (macOS), **Spout** (Windows), **V4L2** (Linux)
- Real-time audio analysis — FFT, beat detection, 8-band spectrum
- Audio-reactive parameter routing — FFT bands → any parameter
- **LFO** modulation (3 banks, 5 waveforms, tempo-sync to BPM)
- **Ableton Link** — join a shared tempo session with Live, Serato, Traktor, etc.
- **ProDJ Link** — BPM, beat phase, and track metadata from CDJ/XDJ/DJM gear
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
│   ├── rustjay-sync        # Ableton Link + ProDJ Link tempo sync (optional)
│   ├── rustjay-gui         # ImGui control window, all built-in tabs
│   ├── rustjay-render      # wgpu pipeline, blit, textures, uniforms
│   └── rustjay-engine      # Facade — app runner, config, re-exports
└── examples/
    ├── template            # HSB colour + full I/O (reference app)
    ├── delta               # RGB delay / motion extraction
    ├── waaaves             # Multi-block feedback pipeline
    └── sputnik             # Indexed mesh + vertex-shader displacement (Rutt-Etra style)
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

## Running the examples

```sh
git clone https://github.com/BlueJayLouche/rustjay-engine
cd rustjay-engine
cargo run -p template    # HSB colour
cargo run -p delta       # RGB delay
cargo run -p waaaves     # feedback pipeline
cargo run -p sputnik     # mesh displacement
```

**Keyboard shortcuts** (output window):
- `Shift+F` — toggle fullscreen
- `Shift+T` — tap tempo
- `Shift+F1–F8` — recall preset quick-slots
- `Escape` — quit

## Platform requirements

| Platform | Notes |
|----------|-------|
| macOS    | Metal GPU. Syphon I/O via [`syphon-rs`](https://github.com/BlueJayLouche/syphon-rs). Requires [Syphon.framework](https://github.com/Syphon/Syphon-Framework) installed in `/Library/Frameworks/`. |
| Windows  | Vulkan or DX12. Spout I/O via DirectX interop. |
| Linux    | Vulkan. V4L2 loopback output. |

NDI requires the [NDI SDK](https://ndi.video/download-ndi-sdk/) installed and the `ndi` feature enabled (default on).

### macOS: installing Syphon

Download and install [Syphon.framework](https://github.com/Syphon/Syphon-Framework/releases) into `/Library/Frameworks/`, or install a Syphon-aware app (Resolume, VDMX, MadMapper) which bundles it.

The Rust bindings come directly from [`BlueJayLouche/syphon-rs`](https://github.com/BlueJayLouche/syphon-rs) — no vendored copy.

## Tempo sync

The engine supports two external sync sources in addition to audio analysis BPM.

### Enabling features

```toml
[dependencies]
rustjay-engine = { git = "https://github.com/BlueJayLouche/rustjay-engine", features = ["link", "prodj"] }
```

Enable one or both. The default build has neither — audio analysis is always available as a fallback.

### Priority

When multiple sources are active the engine picks the highest-priority one:

1. **Ableton Link** — if enabled and at least one peer is present
2. **ProDJ Link** — if enabled and a master deck is present
3. **Audio analysis** — always-on fallback

### Using sync in a plugin

Use `effective_bpm()` and `effective_beat_phase()` instead of `engine.audio.bpm` / `engine.audio.beat_phase`:

```rust
fn build_uniforms(&self, s: &MyState, engine: &EngineState) -> MyUniforms {
    MyUniforms {
        bpm:        engine.effective_bpm(),
        beat_phase: engine.effective_beat_phase(),
        // ...
    }
}
```

The **Sync** tab in the control window lets users enable/disable each source, adjust the Link quantum, and see discovered ProDJ devices — no code changes required.

### Build requirements

- **`link` feature:** CMake ≥ 3.14 must be installed (`brew install cmake` / `apt install cmake`). Links against Ableton Link — the resulting binary is **GPL-2.0+**.
- **`prodj` feature:** No extra system dependencies. Sends LAN broadcast packets and binds UDP ports 50000/50002 — get operator approval before using on a production DJ network.

## Architecture notes

### Device discovery

All I/O device enumeration (webcam, audio, NDI) runs in a background thread so the GPU render loop is never blocked. Syphon server discovery runs on the main thread (the `SyphonServerDirectory` singleton requires it) before the background thread is spawned.

### Thread safety

- `EngineState` is shared via `Arc<Mutex<EngineState>>` between the main thread, GUI, and background services.
- The `Mutex` guard is always held for the minimum scope — especially in `renderer.rs` where the guard is explicitly dropped before the FPS counter sub-lock to avoid same-thread deadlock.

## Roadmap

| Phase | Status | Description |
|-------|--------|-------------|
| 1 | ✅ | Port rustjay-template into a structured workspace |
| 2 | ✅ | `EffectPlugin` trait — bring your own shader and uniforms |
| 3 | ✅ | Multi-input compositor, custom GUI tabs |
| 4 | ✅ | API stabilisation, docs, example gallery |
| 5 | ✅ | Indexed mesh geometry + vertex-shader displacement (sputnik) |
| 6 | ✅ | Ableton Link + ProDJ Link tempo sync |

Stretch goals: hot-reload plugins, GLSL/ISF transpiler, Spout input (Windows), timeline/sequencer.

## License

MIT
