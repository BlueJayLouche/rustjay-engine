# CuePool SeekCue engine design

## Goal

Add `AppCommand::SeekCue { qid, secs }` as the engine contract for the later
scrubbing UI. The command seeks only an active Sound or Video cue, preserves
its playing or paused state, and ignores unknown, inactive, or non-seekable
cues at debug level.

## Audio

Expose the existing `SampleProvider::seek` chain through a public
`MixerInput::seek` passthrough. Positions are interleaved sample counts, so the
command converts seconds to stereo samples using the active output sample rate.
The target is clamped to `[0, length)` with NaN or negative input mapped to
zero; the final valid stereo frame is used for a target at or beyond EOF.

Looped cues use the same loop-relative timeline as `ActiveCueInfo`: zero means
the start of the configured loop region, and the loop duration is the seekable
length. `LoopProcessor::seek` already maps that relative position onto the
trimmed source. Its trim-start lookup must read the command atomic so the same
mapping applies before the first render has refreshed its cache. A target at or
beyond the loop duration lands on its final frame. The command does not change
active state, so a paused input remains silent until resume.

If a tail fade has already started, seeking before its trigger cancels the
in-flight mixer fade, restores the volume at which that fade started, and
clears the bookkeeping flag so the tail fade can trigger again. Seeking into
the fade window restarts the full configured tail fade from that restored
volume; this is deterministic and avoids leaving a seek at an arbitrary
partial gain.
A scheduled `pending_stop` is never changed.

## Video

An active Video cue is identified by `current_video_qid`; its path and declared
duration are copied out of GUI state before any decode or control operation.
The seek target uses the same clamp helper as audio. The decode thread is
restarted through the existing `seek_before` path, which seeks the demuxer to a
keyframe and scans forward to the target region.

The presentation clock is re-anchored so its current position is the requested
target. While playing, normal PTS pacing resumes from that clock and stays
consistent with the separately sought audio input. While paused, the frozen
clock is anchored at the target and the consumer is asked to deliver the first
sought frame, without clearing pause state. The cue-relative target is
translated through a configured media start offset before seeking, while the
displayed clock subtracts that offset again. No GUI-state lock is held across an
audio seek or video decode restart. The decode thread clamps once more against
the container duration it discovers at open, covering an immediate seek on a
silent video whose duration was not available to the main thread yet.

## Verification

Harness integration tests drive a real mixer through `NullSink` and
`RampSource`, checking forward and backward seeks by emitted sample values,
paused behavior, endpoint clamping, and loop-relative seeking. Small pure
helpers in the CuePool binary cover target clamping, video clock arithmetic,
and tail-fade decisions. Full workspace check, clippy, tests, and formatting
run from `examples/cuepool` before the implementation commit.
