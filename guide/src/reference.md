# Keyboard Shortcuts & Feature Flags

## Keyboard shortcuts

These work when the **output window** has focus.

| Key | Action |
|---|---|
| `Shift+F` | Toggle fullscreen |
| `Shift+T` | Tap tempo |
| `Shift+F1` | Recall preset quick-slot 1 |
| `Shift+F2` | Recall preset quick-slot 2 |
| `Shift+F3` | Recall preset quick-slot 3 |
| `Shift+F4` | Recall preset quick-slot 4 |
| `Shift+F5` | Recall preset quick-slot 5 |
| `Shift+F6` | Recall preset quick-slot 6 |
| `Shift+F7` | Recall preset quick-slot 7 |
| `Shift+F8` | Recall preset quick-slot 8 |
| `Escape` | Quit |

## Feature flags

Add any combination to your `Cargo.toml`:

```toml
[dependencies]
rustjay-engine = {
    git = "https://github.com/BlueJayLouche/rustjay-engine",
    features = ["link", "prodj", "mtc", "ndi"]
}
```

| Feature | Description | Extra dependency |
|---|---|---|
| `ndi` | NDI video input and output | NDI SDK installed system-wide |
| `link` | Ableton Link tempo sync | CMake ≥ 3.14; makes binary GPL-2.0+ |
| `prodj` | Pioneer ProDJ Link tempo sync | None (binds UDP 50000/50002) |
| `mtc` | MIDI Timecode receive | None (uses existing `midir` dep) |
| `egui` | egui control backend (alt to ImGui) | None |

Default features: `ndi` is on by default. All others are off.

To disable NDI (e.g. SDK not installed):

```toml
rustjay-engine = { git = "...", default-features = false }
```

## Config file location

Per-app settings (MIDI mappings, last-used preset, OSC port) are stored in:

| Platform | Path |
|---|---|
| macOS / Linux | `~/.config/rustjay/<app-name>.json` |
| Windows | `%APPDATA%\rustjay\<app-name>.json` |

`<app-name>` is the string returned by your `app_name()` implementation.

## OSC parameter paths

All declared parameters are available at:

```
/rustjay/<param-id>   f32
```

Default port: `7770`.

Example — set `intensity` to 0.75:
```
/rustjay/intensity   0.75
```

## Web remote endpoints

Default port: `3000`.

```
GET  /params                   — all parameters (JSON array)
GET  /params/<id>              — single parameter value
POST /params/<id>  {"value": N} — set a parameter
WS   /ws                       — live update stream
```

## Workspace crates

| Crate | Role |
|---|---|
| `rustjay-core` | Shared types: `EffectPlugin`, `EngineState`, LFO, routing |
| `rustjay-audio` | Audio capture, FFT, beat detection |
| `rustjay-io` | Video I/O — webcam, NDI, Syphon, Spout, V4L2 |
| `rustjay-control` | MIDI, OSC, web server |
| `rustjay-presets` | Preset save/load, quick-slots |
| `rustjay-sync` | Ableton Link + ProDJ Link (optional) |
| `rustjay-gui` | ImGui / egui control window |
| `rustjay-render` | wgpu pipeline, textures, uniforms |
| `rustjay-engine` | Facade: `run()`, `run_with_tabs()`, re-exports |
