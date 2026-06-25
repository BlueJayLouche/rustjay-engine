//! Calibration session — the frame-paced loop, as a pure state machine.
//!
//! Owns a [`SequentialCalibrator`] and the settle timing. The caller pumps it
//! once per render frame, handing in the latest captured luma; the session hands
//! back the [`DmxFrame`] to put on the wire and the progress. All I/O (sACN
//! sender, webcam) stays with the caller, so this stays dependency-free and
//! testable with synthetic frames.
//!
//! Loop per frame:
//! ```text
//! let tick = session.tick(latest_luma);   // latest_luma: Option<(&[u8], w, h)>
//! dmx_sender.submit(tick.frame);          // keep driving
//! if tick.done { let map = session.finish(now); map.save(path)?; }
//! ```

use rustjay_lighting::DmxFrame;

use super::calibrate::SequentialCalibrator;
use super::format::LedMap;

/// What to do after a [`CalibrationSession::tick`].
pub struct Tick {
    /// DMX frame to submit this render frame (drives the current LED; blackout
    /// once finished).
    pub frame: DmxFrame,
    /// LED index currently being lit / just finished.
    pub step: u32,
    /// Total LED count.
    pub total: u32,
    /// True once every LED has been recorded.
    pub done: bool,
    /// True while capturing the all-off ambient reference (before any LED is
    /// driven); the caller can show "hold still" and ignore progress.
    pub capturing_reference: bool,
}

/// Drives a [`SequentialCalibrator`] at render-frame pace.
pub struct CalibrationSession {
    cal: SequentialCalibrator,
    /// Render frames to hold each LED lit before sampling (settle / exposure).
    hold_frames: u32,
    /// Frames the current LED has been held so far.
    held: u32,
    /// Ambient reference captured with all LEDs off; subtracted from every frame
    /// before detection. `None` until captured (or if subtraction is disabled).
    background: Option<Vec<u8>>,
    /// True while still capturing the reference, before driving LEDs.
    ref_pending: bool,
    /// Whether background subtraction is enabled.
    subtract: bool,
}

impl CalibrationSession {
    /// Wrap a calibrator. `hold_frames` is how many render frames to keep each
    /// LED lit before capturing — gives the camera time to settle (≈6 at 60fps
    /// ≈ 100ms is a sane start). Clamped to ≥1.
    pub fn new(cal: SequentialCalibrator, hold_frames: u32) -> Self {
        Self { cal, hold_frames: hold_frames.max(1), held: 0, background: None, ref_pending: false, subtract: false }
    }

    /// Like [`new`](Self::new), but first holds all LEDs off for `hold_frames`,
    /// captures that frame as an ambient reference, and subtracts it from every
    /// capture. This removes static lights / room ambient so only the *flashing*
    /// LED is detected — far more accurate when the scene isn't dark.
    pub fn with_background_subtraction(cal: SequentialCalibrator, hold_frames: u32) -> Self {
        Self { cal, hold_frames: hold_frames.max(1), held: 0, background: None, ref_pending: true, subtract: true }
    }

    /// Progress as `(completed, total)`.
    pub fn progress(&self) -> (u32, u32) {
        (self.cal.current_index(), self.cal.count())
    }

    /// Advance one render frame.
    ///
    /// Pass the most recent captured frame as `luma = Some((pixels, w, h))`, or
    /// `None` if no fresh frame is available yet (the session just keeps holding
    /// the current LED until one arrives). Returns the [`DmxFrame`] to submit.
    pub fn tick(&mut self, luma: Option<(&[u8], usize, usize)>) -> Tick {
        let total = self.cal.count();

        // Reference phase: keep LEDs off, capture the ambient frame, then begin.
        if self.ref_pending {
            self.held += 1;
            if self.held >= self.hold_frames
                && let Some((px, _, _)) = luma {
                    self.background = Some(px.to_vec());
                    self.ref_pending = false;
                    self.held = 0;
                }
            return Tick {
                frame: self.cal.blackout_frame(),
                step: 0,
                total,
                done: false,
                capturing_reference: true,
            };
        }

        if self.cal.is_done() {
            // Blackout — empty frame clears nothing actively, so the caller's
            // transport should send zeros; submitting DmxFrame::new() is the
            // "all off" signal.
            return Tick { frame: DmxFrame::new(), step: total, total, done: true, capturing_reference: false };
        }

        self.held += 1;
        if self.held >= self.hold_frames
            && let Some((px, w, h)) = luma {
                match &self.background {
                    // Subtract the ambient reference, then detect on the diff.
                    Some(bg) if self.subtract && bg.len() == px.len() => {
                        let diff: Vec<u8> =
                            px.iter().zip(bg).map(|(p, b)| p.saturating_sub(*b)).collect();
                        self.cal.record(&diff, w, h);
                    }
                    _ => self.cal.record(px, w, h),
                }
                self.held = 0;
            }
            // else: no frame yet — keep holding, try again next tick.

        Tick {
            frame: self.cal.dmx_frame(), // drives whatever LED is now current
            step: self.cal.current_index(),
            total,
            done: self.cal.is_done(),
            capturing_reference: false,
        }
    }

    /// All-zero frame spanning the calibrated universes — submit when stopping
    /// to turn the strip off.
    pub fn blackout(&self) -> DmxFrame {
        self.cal.blackout_frame()
    }

    /// Build the recovered [`LedMap`] (call once [`Tick::done`]). `captured` is
    /// an RFC3339 timestamp string.
    pub fn finish(&self, captured: impl Into<String>) -> LedMap {
        self.cal.finish(captured)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bright frame at a moving spot each tick → completes and maps all LEDs.
    #[test]
    fn drives_to_completion_and_maps() {
        let (w, h) = (10, 10);
        let cal = SequentialCalibrator::new(3, "GRB", 1, 1, 255, 32);
        let mut session = CalibrationSession::new(cal, 1); // capture every tick

        let spots = [(2usize, 2usize), (5, 5), (8, 8)];
        let mut done = false;
        // hold_frames=1 means: tick raises held→1, then records. So one tick per LED.
        for &(sx, sy) in &spots {
            let mut luma = vec![0u8; w * h];
            luma[sy * w + sx] = 255;
            luma[sy * w + sx + 1] = 255;
            let tick = session.tick(Some((&luma, w, h)));
            done = tick.done;
        }
        assert!(done, "should finish after 3 captures");
        assert_eq!(session.progress(), (3, 3));

        let map = session.finish("2026-06-22T00:00:00Z");
        assert_eq!(map.leds.len(), 3);
        assert!(map.leds[0].u < 0.4 && map.leds[2].u > 0.6);
        assert!(map.leds.iter().all(|l| l.conf > 0.0));
    }

    /// `hold_frames` > 1 keeps the same LED lit until enough frames pass.
    #[test]
    fn respects_hold_frames() {
        let cal = SequentialCalibrator::new(2, "RGB", 1, 1, 255, 32);
        let mut session = CalibrationSession::new(cal, 3);
        let luma = vec![0u8; 16]; // dark; detection irrelevant here

        // 2 ticks: still holding LED 0 (held 1,2 < 3).
        session.tick(Some((&luma, 4, 4)));
        assert_eq!(session.progress().0, 0);
        session.tick(Some((&luma, 4, 4)));
        assert_eq!(session.progress().0, 0);
        // 3rd tick records LED 0 and advances.
        session.tick(Some((&luma, 4, 4)));
        assert_eq!(session.progress().0, 1);
    }

    /// Background subtraction removes a static bright source so a new (LED)
    /// light is detected instead of the brighter-or-equal ambient.
    #[test]
    fn background_subtraction_isolates_new_light() {
        let (w, h) = (10, 10);
        let cal = SequentialCalibrator::new(1, "GRB", 1, 1, 255, 32);
        let mut session = CalibrationSession::with_background_subtraction(cal, 1);

        // A static bright "lamp" at top-left present in every frame.
        let mut bg = vec![0u8; w * h];
        bg[w + 1] = 255;
        bg[w + 2] = 255;

        // First tick = reference capture.
        let t = session.tick(Some((&bg, w, h)));
        assert!(t.capturing_reference);
        assert_eq!(session.progress().0, 0);

        // LED 0: lamp + a NEW spot bottom-right. Subtraction leaves only the new.
        let mut frame = bg.clone();
        frame[8 * w + 7] = 255;
        frame[8 * w + 8] = 255;
        let t = session.tick(Some((&frame, w, h)));
        assert!(t.done);

        let map = session.finish("t");
        assert!(
            map.leds[0].u > 0.6 && map.leds[0].v > 0.6,
            "expected the new bottom-right light, got u={} v={}",
            map.leds[0].u,
            map.leds[0].v
        );
    }

    /// No fresh frame → session waits, does not advance past the hold.
    #[test]
    fn waits_when_no_frame() {
        let cal = SequentialCalibrator::new(2, "RGB", 1, 1, 255, 32);
        let mut session = CalibrationSession::new(cal, 1);
        for _ in 0..5 {
            session.tick(None);
        }
        assert_eq!(session.progress().0, 0, "no frames → no progress");
    }
}
