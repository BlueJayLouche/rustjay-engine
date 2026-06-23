# QPlayer — Multi-Output Audio Routing (lightweight) — Design Spec

Goal: **8-channel output** with **per-cue lightweight channel assignment** —
each cue routes its stereo signal to a chosen **output pair** (1-2, 3-4, 5-6,
7-8) with a **send fader**. *Not* a full QLab crosspoint matrix (that's deferred;
the model below is shaped so it can grow into one).

Scope decided 2026-06-23: 8 outs, lightweight assignment, faders.

**Status (2026-06-23): M0–M3 DONE.** 8-ch device open + N-ch mixer
(`mix_stereo_into_pair`), `AudioRouting { out_pair, send }` on Sound/Video,
inspector Output section (pair + send fader), serde save/load. Stereo output
unchanged. **Limitation found:** cues are folded to stereo, so a multichannel
source (e.g. interleaved 5.1) can't route its tracks to discrete outputs — that
needs the **crosspoint matrix** (being built next; see "M4 — later" / Deferred).
The lightweight model is a strict subset of it, so the upgrade is additive.

---

## Current state (what we're changing)

The audio engine is a hard-stereo summing mixer, but most of it is already
channel-generic:

| Piece | Today | N-ch ready? |
|-------|-------|-------------|
| `AudioEngine::new` device select | prefers **F32 stereo** (`channels()==2`) | ❌ must request 8 / max |
| `Mixer::new(channels, sample_rate)` | takes `channels` already | ✅ |
| `Mixer::render` per-cue mix | `apply_volume_pan_mix_stereo` (hard 2-ch) | ❌ the one real change |
| master `Limiter::new(.., channels)` | channel-parameterized | ✅ |
| `MeteringProcessor` | channel-parameterized | ✅ |
| per-cue chain (decode→loop→EQ→fade→resample→mono→stereo→buffered) | outputs **stereo** | ✅ keep — cue is stereo, mixer routes it |

So a cue stays a stereo (L/R) signal; the **mixer places that stereo into the
assigned output pair** of an N-wide buffer. That's the whole idea.

---

## Model (`qplayer-core`)

Add an optional routing to Sound/Video cues (default = pair 0 at unity, so old
shows and the stereo case are unchanged):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AudioRouting {
    /// Output pair index: 0 = outs 1-2, 1 = 3-4, 2 = 5-6, 3 = 7-8.
    #[serde(default)]
    pub out_pair: u8,
    /// Routing send level (linear), the "fader". Default 1.0.
    #[serde(default = "unity")]
    pub send: f32,
}
// Cue::Sound / Cue::Video gain:  #[serde(default)] routing: AudioRouting
```

`pan` stays (L/R balance *within* the assigned pair). `out_pair` clamps to the
available pairs at play time (device may have < 8 outs).

**Forward path to the full matrix (deferred):** replace `out_pair`/`send` with
`Vec<Crosspoint { in_ch: u8, out_ch: u8, gain: f32 }>`. The v1 model is a strict
subset, so migration is additive.

## Mixer (`qplayer-audio`)

1. `MixerInput` gains routing atomics + setter, mirroring `set_pan`:
   ```rust
   out_pair: AtomicU8,        // routing destination pair
   send:     AtomicU32,       // f32 bits, linear
   pub fn set_routing(&self, out_pair: u8, send: f32) { ... }
   ```
2. Replace `apply_volume_pan_mix_stereo(src2, dst2, vol, pan)` with:
   ```rust
   fn mix_cue_into(src_stereo, dst_nch, n_channels, vol, pan, out_pair, send)
   ```
   — apply volume+pan to the cue's L/R (unchanged math), scale by `send`, then
   write into `dst[frame*n + out_pair*2]` (L) and `+1` (R), summing. Other
   output channels untouched for that cue.
3. `Mixer::render` already knows `self.channels`; loop over inputs calling
   `mix_cue_into` instead of the stereo blit.

## Device open (`AudioEngine::new`)

Change the config preference from "F32 stereo" to **"F32 with channels ≥ target
(8), else the max F32 config."** Keep 48 k / device-rate logic. Store the actual
opened `channels`; the routing UI exposes `channels/2` pairs.

## Wiring (`qplayer` binary)

`play_audio` already threads `volume`/`pan` → add `routing: AudioRouting` the
same way (pattern + param + `input.set_routing(routing.out_pair, routing.send)`
right after `set_pan`). Live routing-while-playing is deferred (set at Go), same
as EQ/pan.

## GUI (`qplayer-gui` inspector)

New "Output" section under the existing volume/pan: a **pair selector**
(`1-2 / 3-4 / 5-6 / 7-8`, limited to available pairs) + a **send fader** (dB).
Small, mirrors the EQ section's structure.

---

## Phases

- **M0 — N-channel output.** Open device at 8/max; `mix_cue_into` writes N-wide;
  every cue defaults to pair 0. No audible change for stereo users. *(foundation)*
- **M1 — per-cue routing.** `AudioRouting` model + `set_routing` + clamp; wire
  through `play_audio`.
- **M2 — inspector UI.** Pair selector + send fader.
- **M3 — save/load.** `AudioRouting` serde with defaults (backward-compatible).
- **M4 — later.** Per-output meters, named output **patch** (logical buses →
  physical channels), live routing update, then the **full crosspoint matrix**.

## Defaults / compatibility

- No `routing` in show file → pair 0, send 1.0 (today's behavior).
- Device with < 8 outs → pairs clamp to what exists; out-of-range → pair 0.
- Mono cue → upmixed to stereo (existing) → routed to its pair (both channels).

## Deferred (explicitly out of v1)

Full input×output crosspoint matrix; named/patched output buses; arbitrary
L/R-to-any-channel (non-pair) assignment; live routing edits; per-output
metering UI; master matrix.

## Open questions

1. Send fader range/taper — dB scale (e.g. −∞…+6 dB) and default unity?
2. Should `pan` stay as in-pair balance, or be subsumed once routing exists?
   (Recommend: keep pan — it's the cheap per-cue L/R control.)
3. Master limiter across 8 ch — per-channel (current `Limiter` design) is fine;
   confirm metering shows the main pair vs. a summed/8-ch ladder.
