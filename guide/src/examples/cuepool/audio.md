# Audio

CuePool decodes with [symphonia](https://github.com/pdeljanov/Symphonia)
(all formats enabled — WAV, FLAC, MP3, OGG/Vorbis, AAC, ALAC, …), resamples
to the device rate, and mixes every playing cue through a per-cue DSP chain
(fade → EQ → pan → routing) into a master bus with metering and an optional
limiter. Output is via cpal.

## Project audio settings

*File → Project Settings… → Audio*:

| Setting | Meaning |
|---|---|
| Output device | Any device cpal can see. |
| Latency | Requested output latency in ms (default 10). |
| Channel offset | Shift all output channels — useful on interfaces where outputs 1-2 are not the mains. |
| Exclusive mode / driver | Windows only: WASAPI (default), Wave, DirectSound, ASIO. |
| Limiter | Master-bus brick-wall limiter: input gain, threshold, attack, release. |

## Per-cue level, pan & fades

Every Sound and Video cue has volume (dB), pan, and fade in/out times with a
selectable [curve](cues.md#fade-curves). Stop and Volume cues perform
fade-outs and level rides on already-playing cues.

## Per-cue EQ

Each Sound/Video cue can enable a 4-band parametric EQ plus high-pass and
low-pass filters, edited in the Inspector.

## Routing

`Routing` in the Inspector has two levels:

- **Pair routing** (default) — the cue's stereo signal goes to one output
  pair (*Pair* 0 = outs 1-2, 1 = outs 3-4, …) at a *Send* level.
- **Crosspoint matrix** — add crosspoints to route any source channel to any
  output channel at a gain. This overrides the pair route and handles
  multichannel sources such as 5.1 files.

## Metering & waveform

The transport bar shows the master meter; each entry in **Active Cues** has
its own meter. *Window → Waveform* opens a waveform view of the selected
sound cue — handy when setting start/duration trims.
