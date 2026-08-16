//! LTC chase source — decodes timecode from an audio input into the shared
//! `TimecodeState`, so the follow logic downstream is transport-agnostic.
//!
//! Dropout behavior matches MTC: `playing` clears after 500 ms without a
//! decoded frame, and the follow logic then snaps to the last position and
//! freezes there (see `drive_mtc_follow` in main.rs).

use cuepool_audio::InputCapture;
use cuepool_protocols::ltc::LtcDecoder;
use cuepool_protocols::timecode::{MtcFrameRate, SmpteTime, TimecodeSource, TimecodeState};
use std::time::{Duration, Instant};

/// Mirror of `MtcReceiver`'s transport-stop timeout.
const PLAYING_TIMEOUT: Duration = Duration::from_millis(500);
/// Reconnect attempts while the configured device is missing.
const CONNECT_RETRY: Duration = Duration::from_secs(5);

/// The next timecode frame's label, including drop-frame label skipping
/// (labels 00/01 don't exist at minute starts except every tenth minute).
fn next_frame(tc: &SmpteTime) -> SmpteTime {
    let nominal = match tc.frame_rate {
        MtcFrameRate::Fps2997Drop => 30,
        other => other.fps() as u8,
    };
    let mut next = *tc;
    next.frames += 1;
    if next.frames >= nominal {
        next.frames = 0;
        next.seconds += 1;
        if next.seconds >= 60 {
            next.seconds = 0;
            next.minutes += 1;
            if next.minutes >= 60 {
                next.minutes = 0;
                next.hours = (next.hours + 1) % 24;
            }
        }
    }
    if tc.frame_rate == MtcFrameRate::Fps2997Drop
        && next.seconds == 0
        && next.frames < 2
        && !next.minutes.is_multiple_of(10)
    {
        next.frames = 2;
    }
    next
}

/// How many frames the decoder may lose before a resumed sequence counts as a
/// relocate rather than a dropout. Three covers the bursts a marginal signal
/// produces; a real locate lands far outside this window.
const MAX_LOST_FRAMES: u8 = 3;

/// True when `tc` continues the sequence from `expected`, allowing up to
/// [`MAX_LOST_FRAMES`] frames the decoder failed to produce. Backwards or
/// distant values are not continuations — those are relocates, and go through
/// the two-strike confirmation instead.
fn continues_sequence(expected: &SmpteTime, tc: &SmpteTime) -> bool {
    let mut probe = *expected;
    for _ in 0..=MAX_LOST_FRAMES {
        if probe == *tc {
            return true;
        }
        probe = next_frame(&probe);
    }
    false
}

/// Receives LTC on one configured audio input device.
///
/// The cpal callback only queues samples; decoding happens on the engine
/// thread in [`refresh`](TimecodeSource::refresh), which must be called every
/// frame (it is *not* throttled like `MtcReceiver::refresh` — only the
/// reconnect scan is).
pub struct LtcReceiver {
    device_name: String,
    capture: Option<InputCapture>,
    decoder: Option<LtcDecoder>,
    state: TimecodeState,
    last_frame_at: Option<Instant>,
    /// Expected next frame after a locked sequence (glitch rejection).
    expected: Option<SmpteTime>,
    /// Consecutive frames that broke the expected sequence.
    mismatches: u8,
    last_connect_attempt: Instant,
    /// Drain scratch, kept out of the per-frame path.
    scratch: Vec<f32>,
}

impl LtcReceiver {
    pub fn new(device_name: &str) -> Self {
        Self {
            device_name: device_name.to_string(),
            capture: None,
            decoder: None,
            state: TimecodeState::default(),
            last_frame_at: None,
            expected: None,
            mismatches: 0,
            // So the first refresh() connects immediately (MTC pattern).
            last_connect_attempt: Instant::now() - Duration::from_secs(10),
            scratch: Vec::new(),
        }
    }

    fn connect(&mut self) {
        match InputCapture::start(&self.device_name) {
            Ok(capture) => {
                log::info!(
                    "[LTC] Listening on '{}' at {} Hz",
                    capture.device_name(),
                    capture.sample_rate()
                );
                self.decoder = Some(LtcDecoder::new(capture.sample_rate()));
                self.state.source_device = capture.device_name().to_string();
                self.capture = Some(capture);
                self.expected = None;
                self.mismatches = 0;
            }
            Err(e) => log::warn!("[LTC] Cannot open input: {e}"),
        }
    }

    /// Publish a decoded frame, dropping single-frame glitches: a frame that
    /// does not continue the sequence is adopted only when a second
    /// consecutive mismatch confirms a genuine jump (locate/shuttle).
    ///
    /// "Continues" allows for frames the decoder lost, which is the ordinary
    /// failure on a marginal signal. Requiring an exact +1 would discard the
    /// next *good* frame after every loss, freezing the published position
    /// for three frame periods (~100 ms at 30 fps) — past the follow's 40 ms
    /// deadband, so every isolated dropout would nudge the playback rate.
    fn accept_frame(&mut self, tc: SmpteTime) {
        if let Some(expected) = self.expected
            && !continues_sequence(&expected, &tc)
        {
            self.mismatches += 1;
            if self.mismatches < 2 {
                return;
            }
            log::debug!("[LTC] Jump to {tc}");
        }
        self.mismatches = 0;
        self.expected = Some(next_frame(&tc));
        self.state.running = true;
        self.state.playing = true;
        self.state.position = tc;
        self.last_frame_at = Some(Instant::now());
    }
}

impl TimecodeSource for LtcReceiver {
    fn refresh(&mut self) {
        if self.capture.is_none() && self.last_connect_attempt.elapsed() >= CONNECT_RETRY {
            self.last_connect_attempt = Instant::now();
            self.connect();
        }
        if let (Some(capture), Some(decoder)) = (&self.capture, &mut self.decoder) {
            capture.drain_into(&mut self.scratch);
            for tc in decoder.feed(&self.scratch) {
                self.accept_frame(tc);
            }
            self.scratch.clear();
        }
        if self
            .last_frame_at
            .is_some_and(|at| at.elapsed() > PLAYING_TIMEOUT)
        {
            self.state.playing = false;
        }
    }

    fn tick(&self) {
        // ponytail: no-op — the playing timeout lives in refresh(), which is
        // called every frame anyway; MTC needs tick() only because its state
        // is written from MIDI callback threads.
    }

    fn clone_state(&self) -> TimecodeState {
        self.state.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(frames: u8, rate: MtcFrameRate) -> SmpteTime {
        SmpteTime {
            hours: 1,
            minutes: 0,
            seconds: 0,
            frames,
            frame_rate: rate,
        }
    }

    #[test]
    fn next_frame_rolls_over() {
        let t = next_frame(&tc(24, MtcFrameRate::Fps25));
        assert_eq!((t.seconds, t.frames), (1, 0));
        let t = next_frame(&SmpteTime {
            minutes: 59,
            seconds: 59,
            frames: 23,
            ..tc(0, MtcFrameRate::Fps24)
        });
        assert_eq!((t.hours, t.minutes, t.seconds, t.frames), (2, 0, 0, 0));
        let t = next_frame(&SmpteTime {
            hours: 23,
            minutes: 59,
            seconds: 59,
            frames: 29,
            ..tc(0, MtcFrameRate::Fps30)
        });
        assert_eq!(t.hours, 0);
    }

    #[test]
    fn next_frame_skips_drop_frame_labels() {
        // End of minute 1: 00:01:00:00/01 don't exist — next is :02.
        let end_of_minute = SmpteTime {
            hours: 0,
            minutes: 0,
            seconds: 59,
            frames: 29,
            frame_rate: MtcFrameRate::Fps2997Drop,
        };
        let t = next_frame(&end_of_minute);
        assert_eq!((t.minutes, t.seconds, t.frames), (1, 0, 2));
        // Minute 10 is the exception — labels 00/01 exist.
        let end_of_minute_9 = SmpteTime {
            minutes: 9,
            ..end_of_minute
        };
        let t = next_frame(&end_of_minute_9);
        assert_eq!((t.minutes, t.seconds, t.frames), (10, 0, 0));
    }

    /// A frame the decoder loses must not cost the next good one as well: an
    /// exact +1 rule freezes the position for three frame periods after every
    /// dropout, which is past the follow's deadband.
    #[test]
    fn lost_frames_do_not_stall_the_published_position() {
        let mut rx = LtcReceiver::new("test");
        for frames in 10..=12 {
            rx.accept_frame(tc(frames, MtcFrameRate::Fps25));
        }
        assert_eq!(rx.clone_state().position.frames, 12);

        // The decoder loses 13; 14 arrives and must publish immediately.
        rx.accept_frame(tc(14, MtcFrameRate::Fps25));
        assert_eq!(rx.clone_state().position.frames, 14, "lost frame stalled");
        // A burst of losses inside the window still continues the sequence.
        rx.accept_frame(tc(17, MtcFrameRate::Fps25));
        assert_eq!(rx.clone_state().position.frames, 17, "burst stalled");
        // Beyond the window it is a relocate, so it needs confirming.
        rx.accept_frame(tc(2, MtcFrameRate::Fps25));
        assert_eq!(rx.clone_state().position.frames, 17, "unconfirmed relocate");
        rx.accept_frame(tc(3, MtcFrameRate::Fps25));
        assert_eq!(rx.clone_state().position.frames, 3, "confirmed relocate");
    }

    #[test]
    fn accept_frame_drops_single_glitches_but_adopts_jumps() {
        let mut rx = LtcReceiver::new("test");
        rx.accept_frame(tc(10, MtcFrameRate::Fps25));
        assert_eq!(rx.clone_state().position.frames, 10);
        // In-sequence frame publishes.
        rx.accept_frame(tc(11, MtcFrameRate::Fps25));
        assert_eq!(rx.clone_state().position.frames, 11);
        // A one-frame glitch is dropped...
        rx.accept_frame(tc(20, MtcFrameRate::Fps25));
        assert_eq!(rx.clone_state().position.frames, 11);
        // ...and the sequence recovers.
        rx.accept_frame(tc(12, MtcFrameRate::Fps25));
        assert_eq!(rx.clone_state().position.frames, 12);
        // Two consecutive mismatches = a genuine locate — adopted.
        rx.accept_frame(tc(0, MtcFrameRate::Fps25));
        rx.accept_frame(tc(1, MtcFrameRate::Fps25));
        assert_eq!(rx.clone_state().position.frames, 1);
    }
}
