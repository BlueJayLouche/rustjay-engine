# rustjay-engine

High-performance, cross-platform VJ engine built in Rust — for VJ applications what [Bevy](https://bevyengine.org/) is for games.

**[📖 Guide](https://bluejalouche.github.io/rustjay-engine/)**

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
- Dual-window architecture — Control (ImGui / egui) + Output (fullscreen GPU)

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
│   ├── rustjay-gui         # ImGui + egui control window, all built-in tabs
│   ├── rustjay-render      # wgpu pipeline, blit, textures, uniforms
│   └── rustjay-engine      # Facade — app runner, config, re-exports
├── examples/
│   ├── template            # HSB colour + full I/O (reference app)
│   ├── delta               # RGB delay / motion extraction (ImGui)
│   ├── delta-egui          # Same as delta with egui backend
│   ├── waaaves             # Multi-pass feedback pipeline
│   ├── sputnik             # Indexed mesh + vertex-shader displacement (Rutt-Etra style)
│   ├── isf-example         # Runtime ISF shader loader with auto-generated UI
│   └── webapp              # Web-based control panel (React + WebSocket)
└── guide/                  # mdBook user guide → https://bluejalouche.github.io/rustjay-engine/
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

See the **[guide](https://bluejalouche.github.io/rustjay-engine/)** for a full walkthrough.

## Running the examples

```sh
git clone https://github.com/BlueJayLouche/rustjay-engine
cd rustjay-engine
cargo run -p template      # HSB colour (reference)
cargo run -p delta         # RGB delay / motion extraction
cargo run -p delta-egui    # Same effect, egui backend
cargo run -p waaaves       # Multi-pass feedback pipeline
cargo run -p sputnik       # Mesh displacement (Rutt-Etra style)
cargo run -p isf-example   # Load any .fs ISF shader at runtime
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
| macOS    | Metal GPU. Syphon I/O via [`syphon-rs`](https://github.com/BlueJayLouche/syphon-rs). Requires [Syphon.framework](https://github.com/Syphon/Syphon-Framework) installed in `/Library/Frameworks/`. |
| Windows  | Vulkan or DX12. Spout I/O via DirectX interop. |
| Linux    | Vulkan. V4L2 loopback output. |

NDI requires the [NDI SDK](https://ndi.video/download-ndi-sdk/) installed and the `ndi` feature enabled (default on).

### macOS: installing Syphon

Download and install [Syphon.framework](https://github.com/Syphon/Syphon-Framework/releases) into `/Library/Frameworks/`, or install a Syphon-aware app (Resolume, VDMX, MadMapper) which bundles it.

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

Stretch goals: hot-reload plugins, timeline/sequencer.

## License

MIT
