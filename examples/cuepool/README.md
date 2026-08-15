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

## Command line

```text
cuepool [--zero-copy | --no-zero-copy] [--project <path> | <path>]
```

`--zero-copy` opts into the Windows D3D12VA zero-copy video path;
`--no-zero-copy` forces the stock readback path. The two options are mutually
exclusive. When neither is supplied, the existing `QPLAYER_ZEROCOPY` fallback
is used: the zero-copy path is enabled only when its value is exactly `1`.
Either command-line option takes precedence over that environment variable.

Use `--project <path>` or a single positional path to open a project at startup.

## Automation API

CuePool starts a read-only HTTP API with the app at
`http://127.0.0.1:7133/v1`. The OpenAPI document is available at
`/v1/openapi.json`.

Read endpoints cover health, the loaded project, cues, active cues, the full
Help > Status snapshot, one-second status history, and cursor-based logs.
Log reads accept an optional `limit` from 1 to 1000 for bounded paging.
`/v1/events` is an SSE stream of status samples, new logs, and command results.

Set `CUEPOOL_API_CONTROL_TOKEN` before launch to enable commands. Send the token
as `Authorization: Bearer <token>`. Without it, all reads remain available and
`POST /v1/commands` returns `403 control_disabled`.

```sh
curl http://127.0.0.1:7133/v1/health

curl -H "Authorization: Bearer $CUEPOOL_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"command":"go"}' \
  http://127.0.0.1:7133/v1/commands
```

Commands are `open_project`, `select_cue`, `go`, `stop`, `pause`, `resume`,
`preload`, `seek`, and `shutdown`. The API returns `202` with a pending command ID. Poll
`/v1/commands/{id}` or listen to `/v1/events` for the applied or rejected
result. Send an `Idempotency-Key` header when a command may be retried; reuse of
the same key returns the original command instead of executing it twice while
the result remains in CuePool's 256-command history. Once that result expires,
the old key is rejected rather than executed again.
`open_project` requires an absolute local `.qproj` path and rejects replacement
of a dirty project or a project with active cues.
`shutdown` waits for its final result before returning. It stops only the
CuePool process serving that API, and rejects projects with unsaved changes or
active playback.

`CUEPOOL_API_BIND` can change the loopback address or port, but CuePool rejects
non-loopback binds because the API is plain HTTP. For remote access, forward the
loopback listener through an authenticated TLS tunnel or reverse proxy; never
expose it directly to a network.

MCP clients can use the TypeScript STDIO sidecar in [`mcp/`](mcp/). It maps a
small set of agent-friendly tools onto this API and only advertises control
tools when a token is configured.

### Automation profiles

Set `CUEPOOL_AUTOMATION_PROFILE` to a lowercase name such as `smoke-a` when an
automation run must coexist with another CuePool process. Each named profile
gets its own single-instance lock, recent-file settings, and persistent log.
Use a different `CUEPOOL_API_BIND` port for each running profile.

The default profile is unchanged when the variable is unset or empty. Named
profile settings and logs live below the platform's normal CuePool directory at
`CuePool/automation/<name>/settings.json` and
`CuePool/automation/<name>/cuepool.log`.

### Unattended Windows smoke

Build the exact revision with its commit identity embedded, then run the
PowerShell smoke script from an interactive desktop session:

```powershell
$commit = (git rev-parse --short=7 HEAD).Trim()
$env:CUEPOOL_BUILD_ID = $commit
cargo build --release --locked -p cuepool
# A scheduled task does not inherit Cargo's DLL search path. Make the build
# directory runnable before launching it in an interactive desktop session.
Copy-Item "$env:FFMPEG_DIR\bin\*.dll" .\target\release\

.\scripts\unattended-smoke.ps1 `
  -Executable .\target\release\cuepool.exe `
  -Project C:\content\shows\smoke.qproj `
  -Profile smoke-a `
  -ApiPort 7141 `
  -ExpectedCommit $commit `
  -Artifact C:\content\smoke-results\cuepool-smoke.json `
  -CueQid 1
```

The script selects and plays the cue, checks that active playback blocks
shutdown, pauses, resumes, stops, captures health, GPU diagnostics, status
history and logs, then performs an acknowledged shutdown. It writes one compact
JSON artifact and never stores the generated control token. On failure it stops
only the process it launched.

Process launch remains outside MCP. The smoke artifact proves application and
GPU state, but not physical projector mapping; that still needs an attended or
camera-backed check.

## Window layout

| Area | What it is |
|---|---|
| Top | Menu bar + transport (GO / Stop / Pause, standby readout, master meter) |
| Left | **Active Cues** — every playing cue with state, volume meter, and a progress bar (`elapsed / total  −remaining`; yellow = paused) |
| Center | **Cue list** — the show, in playback order. The standby cue (what GO will fire) carries a chevron in the left gutter and an outlined row; playing cues are green with a ▶ marker, paused cues amber, idle standby blue |
| Right | **Inspector** — full editor for the selected cue |
| Bottom | Status bar — playing-cue count, mode, cue total, unsaved marker, and the live **Video** / **Audio** indicators |

The app has two modes. **Edit** mode enables all editing below; **Show** mode
locks the cue list so a stray click can't rearrange your show mid-performance.

### Video indicator

The status bar names the decode path the current clip is actually using. Green
means the GPU is doing the work and nothing was given up getting there. Amber
with a ⚠ means the path is degraded, either because decoding fell back to the
CPU or because a faster GPU path was abandoned. Hover it for the reason and the
source file; Help > Status has the full picture.

The path is named as the decoder reports it: `hap gpu-native`,
`d3d12va zero-copy (<adapter>)`, `d3d11va readback`, `hardware (videotoolbox)`,
or `software`. A hardware path still reads amber once it has fallen back,
because a clip that lost zero-copy is running slower than it should even though
the GPU is still decoding it.

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
