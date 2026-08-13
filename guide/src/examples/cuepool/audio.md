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
| Output driver | Windows audio host. WASAPI is the default; ASIO uses CPAL's separate ASIO host when the build enables it. |
| Output device | A device enumerated from the selected driver host. The exact name is saved in the project. |
| Latency | Requested output latency in ms (default 10). |
| Channel offset | Shift all output channels — useful on interfaces where outputs 1-2 are not the mains. |
| Exclusive mode | Windows-only output preference. |
| Limiter | Master-bus brick-wall limiter: input gain, threshold, attack, release. |

`Wave` and `DirectSound` are retained as legacy show-file values and use CPAL's
platform-default host, as older CuePool builds did. `ASIO` is the only alternate
CPAL host selected by this setting.

## Building with ASIO on Windows

ASIO is opt-in so normal builds on Windows, macOS, and Linux keep their existing
dependencies:

```powershell
cd examples/cuepool
$env:CPAL_ASIO_DIR = 'C:\SDKs\asiosdk'
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
cargo run --release -p cuepool --features asio
```

Install LLVM (including libclang) and the Steinberg ASIO SDK first.
`CPAL_ASIO_DIR` must point to the extracted SDK root. The feature forwards to
CPAL's `asio` feature and therefore only builds the SDK bindings on Windows.
Enabling `--features asio` on another operating system deliberately adds no
native backend; choosing ASIO there reports that ASIO is Windows-only.

Dante Virtual Soundcard should be running in ASIO mode at 48 kHz with at least
8 x 8 channels. CuePool supports Dante's native 16-, 24-, and 32-bit ASIO
encodings, requests eight output channels, and keeps the existing per-cue
pair/matrix routing. Older CuePool builds based on CPAL 0.15 require DVS's
32-bit encoding as a lossless compatibility workaround for 24-bit audio.

## Output errors

CuePool never substitutes another host or device for a saved non-empty device
name. If ASIO support was not compiled, the ASIO host is unavailable, the saved
driver is missing, or the device cannot open its stream, the Project Settings
window shows the configured driver/device, the reason, and any devices that
were available from that host. Audio playback stays disabled until a valid
output is selected; the project remains open and non-audio cues remain usable.

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
