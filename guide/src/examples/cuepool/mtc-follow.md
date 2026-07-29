# MTC Follow

A video cue with **Follow MTC** enabled plays under MIDI Timecode from an
external master — typically Pro Tools on another machine, sending MTC over
RTP-MIDI. The video plays **silent** (the soundtrack comes from the master),
and CuePool's transport bar gains an `MTC HH:MM:SS.ff` readout (green while
the master's transport is rolling, gray when stopped) with the source port
name and, while a follow cue is active, the current drift.

## What a follow cue does

Enable **Follow MTC** on a video cue in the Inspector and set its
**MTC start** — the MTC position that maps to the first frame of the video
(default `01:00:00:00`, the Pro Tools convention of starting the program at
one hour). Locates before that offset clamp to frame 0.

- **GO** loads the video and holds it on frame 0 until MTC plays.
- **Locate** (master scrubbing / full-frame messages) snaps the video to the
  matching frame and freezes it there.
- **Play** rolls the video in sync; **stop** freezes on the current frame —
  including past the end of the clip (the follow cue never loops or blanks
  on its own; the MTC master owns position).
- While rolling, drift against the master is corrected in three zones:
  under **40 ms** nothing happens (deadband — just presentation jitter);
  **40–250 ms** the playback clock is slewed gently (max 5 % rate, no
  re-seek); over **250 ms** the video hard-seeks to the target.

**Stop All** releases the follow cue; GO on a plain (non-follow) video cue
takes the output back over.

The video must be **25 fps** — CuePool expects 25 fps MTC and logs a warning
if the source sends another rate.

## Wiring it up

### Windows (CuePool machine)

Install [Tobias Erichsen's rtpMIDI](https://www.tobias-erichsen.de/software/rtpmidi.html),
create a session, enable it, and connect to the Mac's session. The session
appears as an ordinary MIDI port — CuePool listens on all MIDI ports, so
there is nothing to select.

### macOS / Pro Tools (master machine)

1. **Audio MIDI Setup → MIDI Studio → Network**: create/enable a network
   session and connect the CuePool machine.
2. **Pro Tools → Setup → Peripherals → Synchronization**: set the
   **MTC generator port** to the network session, then enable MTC generation
   (**Setup → External Sync** / the Transport's sync mode).

### Network

Use **wired ethernet**, not Wi-Fi. MTC itself is light, but Wi-Fi jitter
lands directly in lip-sync — the 40 ms deadband can't absorb a congested
wireless link.

## Testing without Pro Tools

The workspace ships a tiny MTC sender:

```
cargo run --example mtc_send -p cuepool-protocols
```

On one Mac, create an IAC Driver bus in Audio MIDI Setup and pass its name
as the argument. Then type commands:

- `play 01:00:00:00` — stream timecode from one hour (the follow cue starts rolling)
- `locate 01:05:00:00` — jump without playing (the video snaps and holds)
- `stop` — freeze
- `quit`
