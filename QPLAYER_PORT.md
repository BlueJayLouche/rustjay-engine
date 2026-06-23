# QPlayer → rustjay-engine — Port Plan (v1)

QPlayer is an audio-first show-control app (a QLab clone): cue list, multi-cue
audio playback + FX, OSC/MSC remote, video cues, WASM plugins. Goal: a working
release living in this repo as `examples/qplayer`, eventually using the engine's
video/output stack to cue **all sorts of multimedia, like QLab**.

## Decisions (2026-06-23)

| Fork | Choice |
|------|--------|
| **Engine's role** | Video/output superpowers — but **deferred**. v1 has **no `rustjay-engine` dependency**. The engine enters only at the video-cue milestone. |
| **v1 scope** | Audio-first MVP: cue list, audio cues, fades, EQ/pan/limiter, OSC, save/load, undo/redo. |
| **Decode** | **symphonia** (pure Rust). FFmpeg dropped entirely. Video decode is the engine's job later (it already plays video via `hap-wgpu`/`hap-qt`, see `examples/vp404`). |
| **Port state** | Runs, but **audio is very distorted** — see Phase B. |

Why this is consistent, not contradictory: the engine's payoff is concentrated
in *video/output* (NDI/Syphon/Spout/projection). v1 ships the ~85% that's pure
app logic (audio + cues + OSC) as a standalone app; the engine is the
*destination*, reached at the video milestone — not a v1 dependency.

## Status — Phases A + B DONE (2026-06-23)

- **Re-homed** to `examples/qplayer/` as a nested workspace; engine root workspace verified unaffected (resolves clean, pulls in zero qplayer crates).
- **FFmpeg fully removed:** `qplayer-audio` decode → symphonia (`FileDecoder`); `qplayer-video` decode stubbed. No native deps anywhere.
- **Builds in 38.6s**, `cargo build` at `examples/qplayer/`. `cargo test -p qplayer-audio` = 40/40.
- **Runtime confirmed + distortion CONFIRMED FIXED by ear (2026-06-23):** played a 44.1k WAV on the 44.1k device (no-resampler path → directly validates the decoder), "sounds great." Resampler also runs clean at 48k→44.1k.
- **Distortion verdict (settled):** resampler exonerated by unit test; the old hand-rolled FFmpeg decoder was the culprit; symphonia swap fixed it.
- **macOS gotcha:** media in `~/Music`/`~/Desktop`/`~/Documents`/`~/Downloads` is TCC-protected → `File::open` EPERM from a terminal without Full Disk Access. Real releases need FDA or "Pack Project" (P2). Keep test media in unprotected dirs (e.g. `testFiles/`, `/tmp`).

## Status — Phase C (MVP fixes) + video DONE (2026-06-23)

- Cue workflow driven in the GUI; fixed **EQ** (never wired into the play chain +
  inspector left the inner `enabled=false`), **fade-out tail** (C#-parity, starts
  `fade_out`s before the natural end), **resample timing** (loop/trim bounds were
  source-rate vs device-rate `position()`), a **waveform thread-storm crash** (a
  per-frame decode-thread spawn with no in-flight guard exhausted threads on a
  dataless iCloud file), and a **`BufferedSource` thread leak** (bg thread never
  exited).
- **Video RESTORED with FFmpeg — plan correction:** the engine's *own* video
  playback (vjarda's `FfmpegSource`, gated on `rustjay-io/ffmpeg`) is **also
  FFmpeg** under the hood; there is no generic non-FFmpeg decoder (`hap-*` is
  HAP-codec only). So FFmpeg is the correct decoder — restoring it is not a
  regression. The engine's real value is **output** (NDI/Syphon/projection),
  which is separate from decode and remains the add-on. `ffmpeg-next` is back in
  `qplayer-video`; the default build now needs FFmpeg (pkg-config finds it).
- Video cues play in the dual-window blit, **loop** (video-only via `VideoEof`;
  audio-backed via the loop counter), and the output **blanks to black** on
  OneShot end / **holds the last frame** on HoldLast.
- Remaining: optional opt-in `--features video` to keep a lean FFmpeg-free
  default; engine **output routing** (NDI/Syphon/projection).

## Where it lives

`examples/qplayer/` as a **nested workspace** (the engine root lists explicit
members, so it stays invisible to the parent → zero wgpu-25-vs-29 collision).
Keep the existing crate split — it earns its place (real audio-thread isolation,
not speculative abstraction).

## Reuse / swap / defer

| Crate | v1 action |
|-------|-----------|
| `qplayer-core` | **Keep** — cue model = the multimedia-cue backbone. |
| `qplayer-audio` | **Keep DSP, swap `decoder.rs` → symphonia.** Drop `ffmpeg-next`. Mixer/biquad/fade/limiter/pan untouched. |
| `qplayer-gui` | **Keep** (egui — matches newer engine examples). |
| `qplayer` (bin) | **Keep**, fix runtime. |
| `qplayer-video` | **Stubbed, not deleted** — only `video_source.rs` used FFmpeg; replaced with a no-op `VideoSource::open` that errors (`#[ponytail]`), `ffmpeg-next` dropped. Crate kept (lazier than amputating 30+ video sites across the 2042-line `main.rs`, and `VideoSource::open` is the exact seam where engine video plugs in). Video cues log-and-skip. |
| `qplayer-protocols` | OSC **keep**; MSC **deferred** (types stay). |
| `qplayer-plugin-api` | **Deferred** (wasmtime, P3). |
| `ffmpeg-next` | **Removed entirely.** |

## Phases

**A — Resurrect.** Re-home to `examples/qplayer/`, `cargo build`, run, and
document the *actual* broken state (trust runtime over the optimistic README).

**B — Decode swap + chain correctness (the distortion fix).**
- Replace `decoder.rs` (ffmpeg-next → symphonia).
- **The endpoints are already clean** — decoder emits f32 interleaved (SwrContext),
  cpal requests f32 stereo — so **symphonia alone will NOT fix the distortion.**
  It lives downstream:
  - Prime suspect: `ResamplerProcessor` (`rubato::FastFixedOut`, partial-read
    zero-fill + frame accounting). Only engages when source sr ≠ device sr —
    matches "distorted on normal 44.1k files."
  - Secondary: `BufferedSource` ring alignment; `Mixer` sum/headroom.
- Add the one runnable check: 1 kHz sine @44.1k → resample → assert output is
  ~1 kHz with low THD (spectral test behind the DSP).

**C — MVP fix & lock.** v1 surface: cue list; Sound/Stop/Volume/Group/Dummy/
Timecode cues; fades; EQ/pan/limiter; OSC; save/load; undo/redo.

## Deferred (data model stays intact for forward-compat)

VideoCue/ImageCue runtime, MSC, WASM plugins, remote nodes, Pack Project.

## Guardrail

Keep `Cue` an **open enum/trait**, and keep `VideoCue`/`ImageCue` in the serde
model now even though their runtime is unimplemented. Then v1 show-files are
forward-compatible and the engine video arm is *additive*, not a rewrite.
`ImageCue` (static texture → engine output) is the cheapest first engine cue —
the natural Phase D smoke test before VideoCue.
