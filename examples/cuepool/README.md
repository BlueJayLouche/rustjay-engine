# CuePool

A theatre sound/video cue player — a Rust port of [QPlayer](https://github.com/space928/QPlayer)
(QLab-style show control), built on the rustjay engine. Renamed CuePool to avoid
confusion with the original project. Audio via symphonia/cpal, video via FFmpeg,
plus OSC/MIDI show control, projection-mapped video outputs, and lighting cues
(sACN / Art-Net).

This is a standalone nested workspace: build from this directory, not the repo root.

```sh
cd examples/cuepool
cargo run --release
```

## Window layout

| Area | What it is |
|---|---|
| Top | Menu bar + transport (GO / Stop / Pause, standby readout, master meter) |
| Left | **Active Cues** — every playing cue with state, volume meter, and a progress bar (`elapsed / total  −remaining`; yellow = paused) |
| Center | **Cue list** — the show, in playback order. The standby cue (what GO will fire) carries a chevron in the left gutter and an outlined row; playing cues are green with a ▶ marker, paused cues amber, idle standby blue |
| Right | **Inspector** — full editor for the selected cue |
| Bottom | Status bar |

The app has two modes. **Edit** mode enables all editing below; **Show** mode
locks the cue list so a stray click can't rearrange your show mid-performance.

## Editing the cue list (Edit mode)

- **Rename / renumber inline** — the `#` and `Name` cells are text fields; click
  and type. Cue numbers commit when the field loses focus (Enter or click away;
  Esc cancels), names commit as you type. Renumbering follows references: group
  members, Stop/Volume/Goto cues targeting the old number, and the selection all
  move to the new number. Duplicate numbers are rejected.
- **Add cues** — toolbar buttons above the list, or right-click → *Add … Cue*.
  New cues are numbered after the selected cue.
- **Right-click menu** — Move Up/Down, Duplicate, Delete, Add cue.
- **Reorder / group** — drag the `≡` handle. Drop a cue onto a Group (or one of
  its members) to join the group — members draw indented under the group header.
  Drop on the strip below the list to ungroup / move to the end.
- **Delete** — right-click → Delete, or select and press Delete/Backspace.

## Keyboard shortcuts

| Key | Action |
|---|---|
| Space | GO (fire the standby cue) |
| Esc | Stop all |
| ↑ / ↓ | Move the standby cue up / down the list |
| Home / End | Standby the first / last cue |
| Cmd/Ctrl+Z / Shift+Z | Undo / Redo |
| Cmd/Ctrl+N / O / S | New / Open / Save project |
| Cmd/Ctrl+T | Add sound cue |
| Cmd/Ctrl+D | Duplicate selected cue |
| Cmd/Ctrl+↑ / ↓ | Move selected cue up / down |
| Delete / Backspace | Delete selected cue |

## Cue types

Sound, Video, Image, Text, Group, Stop, Volume, Dummy, TimeCode, OSC, Goto,
Lighting, PixelMap. Each cue has a trigger mode: **Go** (waits for GO),
**WithLast** (fires with the previous cue), **AfterLast** (fires when the
previous cue finishes).

## Projects

Projects are JSON `.qproj` files. *File → Pack Project* copies all referenced
media next to the project file for touring. OSC receive/transmit ports and the
network interface live in Project Settings (defaults: rx 9000 / tx 9001).
