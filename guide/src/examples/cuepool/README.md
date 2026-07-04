# CuePool — Theatre Show Control

`examples/cuepool` is a theatre sound, video, and lighting cue player — a
QLab-style show-control app, ported to Rust from
[QPlayer](https://github.com/space928/QPlayer) and renamed CuePool to avoid
confusion with the original project. You build a list of cues (sound, video,
lighting, OSC, …), then fire them one after another with the GO button during
a performance.

Under the hood: audio decode via symphonia with a custom DSP chain on cpal,
video via FFmpeg with GPU YUV→RGB conversion, DMX over sACN or Art-Net, and
OSC / MIDI show control.

CuePool is a standalone nested workspace — build it from its own directory,
not the repo root:

```sh
cd examples/cuepool
cargo run --release
```

## The main window

| Area | What it is |
|---|---|
| Top | Menu bar + transport (GO / Stop / Pause, master meter) |
| Left | **Active Cues** — every playing cue with state, volume meter, and a progress bar (`elapsed / total  −remaining`; yellow = paused) |
| Center | **Cue list** — the show, in playback order |
| Right | **Inspector** — full editor for the selected cue |
| Bottom | Status bar |

Extra panels live in the **Window** menu: Log, Waveform, Video Output,
Projection Mapping, and Lighting.

## Edit mode vs Show mode

The app has two modes. **Edit** mode enables all editing; **Show** mode locks
the cue list so a stray click can't rearrange your show mid-performance.

## Projects

Projects are JSON `.qproj` files (the file format is compatible with the
original QPlayer). While a project is dirty, an autosave thread writes a
backup every 60 seconds, rotating through five slots
(`autoback_1.qproj` … `autoback_5.qproj` in the per-user config directory),
and a crash handler saves `crash_recovery.qproj` on the way down. *File →
Pack…* copies all referenced media next to the project file for touring.

## Chapters

- [Getting Started](getting-started.md) — build, run, and program your first show
- [Cue Reference](cues.md) — every cue type and its properties
- [Audio](audio.md) — devices, routing, EQ, fades, and the limiter
- [Video & Projection](video.md) — the canvas, output windows, and edge blending
- [Lighting & Pixel Mapping](lighting.md) — DMX patch, lighting cues, and LED segments
- [Show Control](show-control.md) — OSC, MIDI/MSC, hotkeys, timecode, and remote nodes

## Where to look (for developers)

| Area | Crate / module |
|---|---|
| App binary, event loop, playback engine | `crates/cuepool/src/main.rs` |
| Cue model, show file, projection & lighting config | `crates/cuepool-core` |
| Audio engine (decode, DSP, mixer) | `crates/cuepool-audio` |
| Video decode + projection renderer | `crates/cuepool-video` |
| egui panels (cue list, inspector, transport, …) | `crates/cuepool-gui` |
| OSC, MIDI, and MSC | `crates/cuepool-protocols` |
