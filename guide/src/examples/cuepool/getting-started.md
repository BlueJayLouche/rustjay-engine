# Getting Started

## Build & run

CuePool needs a Rust toolchain and the FFmpeg libraries (for video decode):

```sh
# macOS
brew install ffmpeg

# then, from the repo:
cd examples/cuepool
cargo run --release
```

On Windows, `package-windows.ps1` builds and bundles the app with its FFmpeg
DLLs. You can also open a project straight from the command line:

```sh
cargo run --release -- MyShow.qproj
```

CuePool is single-instance: launching a second copy while one is running
exits immediately.

## Your first show

1. **New project** — `Cmd/Ctrl+N`, then *File → Project Settings…* to set the
   show title, author, and audio output device.
2. **Add a sound cue** — `Cmd/Ctrl+T`, a toolbar button above the cue list, or
   right-click → *Add Sound Cue*. Pick a file in the Inspector (*File:*).
3. **Shape it** — still in the Inspector: volume (dB), pan, fade in/out
   seconds, and the fade curve. The *Start* / *Duration* fields trim the clip.
4. **GO** — press `Space`. The cue appears in **Active Cues** on the left with
   a meter and progress bar. `Esc` stops everything.
5. **Build a sequence** — set the next cue's trigger to **WithLast** (fires
   together with the previous cue) or **AfterLast** (fires when it finishes),
   optionally with a *Delay*. A single GO then plays the whole chain.
6. **Save** — `Cmd/Ctrl+S` writes a `.qproj` file. *File → Pack…* copies all
   referenced media next to the project file for touring.

## Editing the cue list (Edit mode)

- **Rename / renumber inline** — the `#` and `Name` cells are text fields;
  click and type. Cue numbers commit when the field loses focus (Enter or
  click away; Esc cancels), names commit as you type. Renumbering follows
  references: group members, Stop/Volume/Goto cues targeting the old number,
  and the selection all move to the new number. Duplicate numbers are
  rejected.
- **Add cues** — toolbar buttons above the list, or right-click → *Add … Cue*.
  New cues are numbered after the selected cue by decimal subdivision
  (after cue 1 comes 1.1, then 1.2, …).
- **Right-click menu** — Move Up/Down, Duplicate, Delete, Add cue.
- **Reorder / group** — drag the `≡` handle. Drop a cue onto a Group (or one
  of its members) to join the group — members draw indented under the group
  header. Drop on the strip below the list to ungroup / move to the end.
- **Delete** — right-click → Delete, or select and press Delete/Backspace.

## Keyboard shortcuts

| Key | Action |
|---|---|
| Space | GO (fire next cue) |
| Esc | Stop all |
| Cmd/Ctrl+Z / Shift+Z | Undo / Redo |
| Cmd/Ctrl+N / O / S | New / Open / Save project |
| Cmd/Ctrl+T | Add sound cue |
| Cmd/Ctrl+D | Duplicate selected cue |
| Cmd/Ctrl+↑ / ↓ | Move selected cue up / down |
| Delete / Backspace | Delete selected cue |

Individual cues can also have their own [hotkey, MIDI, wall-clock, and
timecode triggers](show-control.md#per-cue-triggers).
