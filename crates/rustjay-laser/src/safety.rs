//! What stands between a shader and the beam.
//!
//! Two things go wrong here that do not go wrong with pixels. A projector can
//! be streaming before anyone meant it to be, and a shader can produce a frame
//! that is technically valid but physically dangerous — `pos = vec2(0.0)` for
//! every sample is a live beam held on one spot, and it is a one-character bug.
//!
//! So nothing streams until it is armed, and every frame is checked on the way
//! out. The check is cheap because the points are already on the CPU: the beam
//! has to be moving across some area, or the frame goes out dark.
//!
//! The thresholds here are conventional starting values, **not verified against
//! hardware**. They should be checked against a real projector before this
//! drives one, and they are deliberately conservative in the direction of
//! blanking a frame that might have been fine.

use crate::frame::LaserFrame;

/// Smallest extent, in scan-field units, that counts as the beam moving.
///
/// The field is 2 across, so this is half a percent of it — a beam confined to
/// less is a dot however it got there.
pub const MIN_LIT_EXTENT: f32 = 0.01;

/// Why a frame was not sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// Nobody has armed the output.
    Disarmed,
    /// The shader did not compile, so there is nothing trustworthy to send.
    ShaderError,
    /// The lit points cover almost no area — a parked or near-parked beam.
    ScanFail,
}

impl Blocked {
    /// A line for the UI to show next to the output.
    pub fn reason(&self) -> &'static str {
        match self {
            Blocked::Disarmed => "output disarmed",
            Blocked::ShaderError => "shader error — output held dark",
            Blocked::ScanFail => "scan fail: beam is not moving — output held dark",
        }
    }
}

/// The gate every frame passes through before reaching a DAC.
#[derive(Clone, Debug)]
pub struct Safety {
    armed: bool,
    /// Extent below which a frame is treated as a parked beam.
    pub min_lit_extent: f32,
    /// Why the last frame was blocked, if it was.
    pub blocked: Option<Blocked>,
}

impl Default for Safety {
    fn default() -> Self {
        Self { armed: false, min_lit_extent: MIN_LIT_EXTENT, blocked: Some(Blocked::Disarmed) }
    }
}

impl Safety {
    /// Allow output. Deliberately explicit and deliberately not persisted:
    /// starting the app must never start a laser.
    pub fn arm(&mut self) {
        self.armed = true;
        self.blocked = None;
    }

    /// Stop output. The panic path — cheap, immediate, always available.
    pub fn disarm(&mut self) {
        self.armed = false;
        self.blocked = Some(Blocked::Disarmed);
    }

    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// The frame to send, or `None` to send nothing.
    ///
    /// `shader_ok` is false when the material failed to compile — a stale frame
    /// from before the edit is not something to keep drawing.
    pub fn gate<'a>(&mut self, frame: &'a LaserFrame, shader_ok: bool) -> Option<&'a LaserFrame> {
        self.blocked = if !self.armed {
            Some(Blocked::Disarmed)
        } else if !shader_ok {
            Some(Blocked::ShaderError)
        } else if frame.lit_extent().is_some_and(|e| e < self.min_lit_extent) {
            Some(Blocked::ScanFail)
        } else {
            None
        };
        self.blocked.is_none().then_some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::LaserPoint;

    fn lit(x: f32, y: f32) -> LaserPoint {
        LaserPoint { x, y, r: 1.0, g: 1.0, b: 1.0, shape: 0 }
    }

    fn line() -> LaserFrame {
        LaserFrame { points: vec![lit(-0.5, 0.0), lit(0.5, 0.0)] }
    }

    #[test]
    fn nothing_goes_out_until_it_is_armed() {
        let mut safety = Safety::default();

        assert!(safety.gate(&line(), true).is_none());
        assert_eq!(safety.blocked, Some(Blocked::Disarmed));

        safety.arm();
        assert!(safety.gate(&line(), true).is_some());
        assert_eq!(safety.blocked, None);
    }

    #[test]
    fn disarming_stops_output_immediately() {
        let mut safety = Safety::default();
        safety.arm();
        assert!(safety.gate(&line(), true).is_some());

        safety.disarm();

        assert!(safety.gate(&line(), true).is_none());
    }

    // The failure this exists for: every sample at the same place.
    #[test]
    fn a_parked_beam_is_blocked_even_when_armed() {
        let mut safety = Safety::default();
        safety.arm();
        let parked = LaserFrame { points: vec![lit(0.0, 0.0); 500] };

        assert!(safety.gate(&parked, true).is_none());
        assert_eq!(safety.blocked, Some(Blocked::ScanFail));
    }

    // A tight scribble concentrates as much energy as a dot does.
    #[test]
    fn a_beam_confined_to_a_tiny_area_is_blocked() {
        let mut safety = Safety::default();
        safety.arm();
        let scribble = LaserFrame {
            points: (0..500)
                .map(|i| {
                    let t = i as f32 * 0.3;
                    lit(t.cos() * 0.002, t.sin() * 0.002)
                })
                .collect(),
        };

        assert_eq!(safety.gate(&scribble, true).map(|_| ()), None);
        assert_eq!(safety.blocked, Some(Blocked::ScanFail));
    }

    // Blanked points are not a hazard wherever they are, so a frame that only
    // travels dark is not a scan fail — it is simply nothing.
    #[test]
    fn a_fully_blanked_frame_passes_because_it_emits_nothing() {
        let mut safety = Safety::default();
        safety.arm();
        let dark = LaserFrame { points: vec![LaserPoint::blanked(0.0, 0.0, 0); 500] };

        assert!(safety.gate(&dark, true).is_some());
    }

    #[test]
    fn a_broken_shader_holds_the_output_dark() {
        let mut safety = Safety::default();
        safety.arm();

        assert!(safety.gate(&line(), false).is_none());
        assert_eq!(safety.blocked, Some(Blocked::ShaderError));
    }

    // Arming is a decision someone makes, never a state that is restored.
    #[test]
    fn a_fresh_gate_is_disarmed() {
        assert!(!Safety::default().is_armed());
    }
}
