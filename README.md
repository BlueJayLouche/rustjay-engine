# rustjay-engine

High-performance, cross-platform VJ engine built in Rust — for VJ applications what [Bevy](https://bevyengine.org/) is for games.

**[📖 Guide](https://BlueJayLouche.github.io/rustjay-engine/)**

## What is it?

rustjay-engine is a Cargo workspace of focused crates that handles all the infrastructure of a live visual performance tool:

- GPU rendering via **wgpu** (Metal / Vulkan / DX12)
- Video I/O — **Webcam**, **NDI**, **Syphon** (macOS), **Spout** (Windows), **V4L2** (Linux)
- **DeckLink** capture input (Blackmagic)
- Real-time audio analysis — FFT, beat detection, 8-band spectrum
- Audio-reactive parameter routing — FFT bands → any parameter
- **LFO** modulation (3 banks, 5 waveforms, tempo-sync to BPM)
- **Ableton Link** — join a shared tempo session with Live, Serato, Traktor, etc.
- **ProDJ Link** — BPM, beat phase, and track metadata from CDJ/XDJ/DJM gear
- **MIDI** learn and mapping
- **OSC** server
- **Web** parameter server (REST + live push)
- **Presets** with quick-slots (Shift+F1–F8)
- **Multi-channel mixer** — N-deck compositing with FX chains and scene persistence
- **Projection mapping** — output post-processor with dome, warp, edge-blend, slicer
- **DMX lighting output** — sACN / Art-Net with per-fixture pixel sampling
- **ISF shaders** — load any Interactive Shader Format `.fs` at runtime
- Dual-window architecture — Control (ImGui / egui) + Output (fullscreen GPU)

You bring the shader and the idea. The engine handles everything else.

## Workspace layout

```
rustjay-engine/
├── crates/
│   ├── rustjay-core        # EngineState, command enums, LFO, routing types
│   ├── rustjay-audio       # AudioAnalyzer, FFT, beat detection
│   ├── rustjay-io          # InputManager, OutputManager (Webcam/NDI/Syphon/Spout/V4L2/DeckLink)
│   ├── rustjay-control     # MIDI, OSC, Web server
│   ├── rustjay-presets     # Preset save/load/quick-slots
│   ├── rustjay-sync        # Ableton Link + ProDJ Link + MTC tempo sync (optional)
│   ├── rustjay-gui         # ImGui + egui control window, all built-in tabs
│   ├── rustjay-render      # wgpu pipeline, blit, textures, uniforms
│   ├── rustjay-mixer       # Multi-channel compositing mixer with FX chains
│   ├── rustjay-projection  # Output post-processor (dome, warp, edge-blend, slicer)
│   ├── rustjay-lighting    # DMX lighting output — sACN / Art-Net pixel sampling
│   ├── rustjay-isf         # ISF shader support — GLSL→WGSL transpiler + EffectPlugin adapter
│   ├── rustjay-api         # Optional REST/OpenAPI layer
│   └── rustjay-engine      # Facade — app runner, config, re-exports
├── examples/
│   ├── template            # HSB colour + full I/O (reference app)
│   ├── delta               # RGB delay / motion extraction (ImGui)
│   ├── delta-egui          # Same as delta with egui backend
│   ├── flux                # Optical-flow warp with motion feedback trails
│   ├── waaaves             # Multi-pass feedback pipeline
│   ├── sputnik             # Indexed mesh + vertex-shader displacement (Rutt-Etra style)
│   ├── isf-example         # Runtime ISF shader loader with auto-generated UI
│   ├── mixer               # 2-channel mixer demonstrating rustjay-mixer
│   ├── projection          # Projection mapping demonstrating rustjay-projection
│   ├── decklink            # Blackmagic DeckLink capture input
│   ├── vjarda              # Full multi-deck VJ application
│   └── webapp              # Web-based control panel (React + WebSocket / WASM + WebGPU)
└── guide/                  # mdBook user guide → https://BlueJayLouche.github.io/rustjay-engine/
```

## Quick start

```toml
# Cargo.toml
[dependencies]
rustjay-engine = { git = "https://github.com/BlueJayLouche/rustjay-engine" }
bytemuck = { version = "1.21", features = ["derive"] }
serde = { version = "1.0", features = ["derive"] }
anyhow = "1.0"
env_logger = "0.11"
log = "0.4"
```

```rust
// src/main.rs
use rustjay_engine::prelude::*;

struct MyEffect;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct MyUniforms { intensity: f32 }

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct MyState { intensity: f32 }

impl EffectPlugin for MyEffect {
    type State    = MyState;
    type Uniforms = MyUniforms;

    fn app_name(&self) -> &str { "my-effect" }
    fn shader_source(&self) -> &'static str { include_str!("shaders/my_effect.wgsl") }
    fn build_uniforms(&self, s: &MyState, _e: &EngineState) -> MyUniforms {
        MyUniforms { intensity: s.intensity }
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    rustjay_engine::run(MyEffect)
}
```

That's it — two windows open: a fullscreen GPU output and a tabbed control panel with input, audio, LFO, MIDI, OSC, presets, and output built in.

See the **[guide](https://BlueJayLouche.github.io/rustjay-engine/)** for a full walkthrough.

## Running the examples

```sh
git clone https://github.com/BlueJayLouche/rustjay-engine
cd rustjay-engine
cargo run -p template      # HSB colour (reference)
cargo run -p delta         # RGB delay / motion extraction
cargo run -p delta-egui    # Same effect, egui backend
cargo run -p flux          # Optical-flow warp
cargo run -p waaaves       # Multi-pass feedback pipeline
cargo run -p sputnik       # Mesh displacement (Rutt-Etra style)
cargo run -p isf-example   # Load any .fs ISF shader at runtime
cargo run -p mixer         # 2-channel compositor
cargo run -p projection    # Projection mapping
cargo run -p vjarda        # Full multi-deck VJ app (--all-features for NDI/Syphon/Spout)
cargo run -p webapp        # Web control panel (open http://localhost:3000)
```

**Keyboard shortcuts** (output window):
- `Shift+F` — toggle fullscreen
- `Shift+T` — tap tempo
- `Shift+F1–F8` — recall preset quick-slots
- `Escape` — quit

## Platform requirements

| Platform | Notes |
|----------|-------|
| macOS    | Metal GPU. Syphon I/O via [`syphon-core`](https://crates.io/crates/syphon-core) / [`syphon-wgpu`](https://crates.io/crates/syphon-wgpu) 0.2 — framework bundled, no separate install needed. |
| Windows  | Vulkan or DX12. Spout I/O via DirectX interop. |
| Linux    | Vulkan. V4L2 loopback output. |

NDI requires the [NDI SDK](https://ndi.video/download-ndi-sdk/) installed and the `ndi` feature enabled (default on).

## Tempo sync

The engine supports three tempo sources, selected by the user at runtime from the Audio tab.

### Enabling features

```toml
rustjay-engine = { git = "...", features = ["link", "prodj", "mtc"] }
```

| Source | Feature | What it uses |
|--------|---------|-------------|
| **Audio / Tap Tempo** | _(always on)_ | Real-time beat detection from audio input, or tap-tempo |
| **Ableton Link** | `link` | Joins the local Link session — syncs with Live, Serato, Traktor, etc. |
| **ProDJ Link** | `prodj` | BPM, beat phase, and track metadata from CDJ/XDJ/DJM gear |
| **MIDI Timecode** | `mtc` | SMPTE position reference from any connected DAW |

### Using sync in a plugin

Use `effective_bpm()` and `effective_beat_phase()` — they dispatch on whichever source is active:

```rust
fn build_uniforms(&self, s: &MyState, engine: &EngineState) -> MyUniforms {
    MyUniforms {
        bpm:        engine.effective_bpm(),
        beat_phase: engine.effective_beat_phase(),
    }
}
```

### Build requirements

- **`link`:** CMake ≥ 3.14 (`brew install cmake`). Links against Ableton Link — binary becomes **GPL-2.0+**.
- **`prodj`:** No extra deps. Binds UDP 50000/50002 — get operator approval on production DJ networks.
- **`mtc`:** No extra deps.

## Architecture notes

- `EngineState` is shared via `Arc<Mutex<EngineState>>` between the main thread, GUI, and background services
- All I/O device enumeration (webcam, audio, NDI) runs in a background thread — GPU render loop never blocks
- Syphon server discovery runs on the main thread (singleton requirement) before the background thread spawns

## Roadmap

| Phase | Status | Description |
|-------|--------|-------------|
| 1 | ✅ | Port rustjay-template into a structured workspace |
| 2 | ✅ | `EffectPlugin` trait — bring your own shader and uniforms |
| 3 | ✅ | Multi-input compositor, custom GUI tabs |
| 4 | ✅ | API stabilisation, docs, example gallery |
| 5 | ✅ | Indexed mesh geometry + vertex-shader displacement (sputnik) |
| 6 | ✅ | Ableton Link + ProDJ Link tempo sync |
| SG-6 | ✅ | MIDI Timecode, explicit sync source selector, LFO beat-phase fix |
| 7 | ✅ | ISF shader viewer, web remote, egui backend, user guide |
| 8 | ✅ | Windows support — Spout I/O, NDI robustness, CI |
| 9 | ✅ | Multi-deck VJ app (vjarda) — mixer, FX chains, scene topology persistence |
| 10 | ✅ | Projection mapping — output post-processor, headless NDI/Syphon/Spout/V4L2 sinks |
| 11 | ✅ | DMX lighting output — sACN / Art-Net with per-fixture pixel sampling |

Stretch goals: hot-reload plugins, timeline/sequencer, VARDA full-parity port.

## License

MIT
