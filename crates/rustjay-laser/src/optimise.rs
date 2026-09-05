//! Turning a material's point list into something a galvanometer can track.
//!
//! A shader emits geometry: samples spread evenly along a path, with a shape
//! number marking where one stroke ends and the next begins. Mirrors driven by
//! that directly overshoot every corner and draw a bright line across every
//! jump, because a mirror has mass and the point list has no idea.
//!
//! Three insertions fix most of it, and they are the three MadMapper's
//! `RENDER_SETTINGS` spend most of their knobs on:
//!
//! - **blanking** — held, dark points at both ends of a jump between strokes,
//!   so the beam is off while it travels and has settled before it lights;
//! - **corner dwell** — repeated points where the path turns sharply, giving
//!   the mirrors time to change direction instead of rounding it off;
//! - **stroke repeats** — repeated points at each end of a stroke, so it starts
//!   and finishes where it should rather than smearing in and out.
//!
//! ponytail: three of MadMapper's nine settings. The interpolation ones
//! (`MAX_SPEED`), draw-order reordering (`PRESERVE_ORDER`) and the fades are
//! not here — the shader already controls point density along the path, so
//! there is nothing to interpolate, and reordering only pays off with many
//! strokes. Upgrade path is a full scan optimiser; `nannou_laser` has one
//! implementing Abderyim et al. if this stops being enough.
//!
//! None of these numbers have been checked against real mirrors. They are the
//! conventional starting values; a projector will say what they should be.

use crate::frame::{LaserFrame, LaserPoint};

/// How much settling to insert, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Optimiser {
    /// Dark points held at each end of a jump between strokes.
    pub blank_dwell: usize,
    /// Extra copies of a point where the path turns sharply.
    pub corner_dwell: usize,
    /// Extra copies at the start and end of every stroke.
    pub stroke_repeat: usize,
    /// Turn, in radians, above which a vertex counts as a corner. MadMapper's
    /// documented default for `ANGLE_THRESHOLD` is 0.5 — about 29 degrees.
    pub corner_threshold: f32,
    /// Drop strokes that emit no light instead of scanning through them.
    pub skip_black: bool,
}

impl Default for Optimiser {
    fn default() -> Self {
        Self {
            blank_dwell: 4,
            corner_dwell: 2,
            stroke_repeat: 3,
            corner_threshold: 0.5,
            skip_black: false,
        }
    }
}

impl Optimiser {
    /// Take the settings a material asked for, where it asked for any.
    pub fn from_render_settings(settings: &rustjay_isf::header::RenderSettings) -> Self {
        Self {
            // A material that turns angle optimisation off wants its corners
            // drawn as given — a circle gains nothing from dwelling at each of
            // its 500 vertices, and pays for every one.
            corner_dwell: match settings.angle_optimization {
                Some(false) => 0,
                _ => Self::default().corner_dwell,
            },
            skip_black: settings.skip_black.unwrap_or(false),
            ..Self::default()
        }
    }

    /// Rewrite a frame with the settling points a scanner needs.
    pub fn run(&self, frame: &LaserFrame) -> LaserFrame {
        let mut out: Vec<LaserPoint> = Vec::with_capacity(frame.points.len() * 2);

        for stroke in frame.strokes() {
            let points = &frame.points[stroke];
            let (Some(first), Some(last)) = (points.first(), points.last()) else {
                continue;
            };
            if self.skip_black && points.iter().all(LaserPoint::is_blank) {
                continue;
            }

            // Travel to the new stroke dark: hold where we were, then hold at
            // the new start before lighting it.
            if let Some(previous) = out.last().copied() {
                push_n(&mut out, LaserPoint::blanked(previous.x, previous.y, first.shape), self.blank_dwell);
                push_n(&mut out, LaserPoint::blanked(first.x, first.y, first.shape), self.blank_dwell);
            }

            push_n(&mut out, *first, self.stroke_repeat);
            for (i, p) in points.iter().enumerate() {
                out.push(*p);
                if self.corner_dwell > 0 && is_corner(points, i, self.corner_threshold) {
                    push_n(&mut out, *p, self.corner_dwell);
                }
            }
            push_n(&mut out, *last, self.stroke_repeat);
        }

        // Leave the beam dark rather than parked lit on the final point.
        if let Some(last) = out.last().copied() {
            push_n(&mut out, LaserPoint::blanked(last.x, last.y, last.shape), self.blank_dwell);
        }
        LaserFrame { points: out }
    }
}

/// Whether the path turns sharply at `i`. Ends of a stroke are not corners —
/// they already get [`Optimiser::stroke_repeat`].
fn is_corner(points: &[LaserPoint], i: usize, threshold: f32) -> bool {
    if i == 0 || i + 1 >= points.len() {
        return false;
    }
    let (prev, here, next) = (points[i - 1], points[i], points[i + 1]);
    let incoming = (here.x - prev.x, here.y - prev.y);
    let outgoing = (next.x - here.x, next.y - here.y);
    let lengths = incoming.0.hypot(incoming.1) * outgoing.0.hypot(outgoing.1);
    if lengths <= f32::EPSILON {
        return false; // a stationary vertex has no direction to turn from
    }
    let cos = ((incoming.0 * outgoing.0 + incoming.1 * outgoing.1) / lengths).clamp(-1.0, 1.0);
    cos.acos() > threshold
}

fn push_n(out: &mut Vec<LaserPoint>, point: LaserPoint, n: usize) {
    out.extend(std::iter::repeat_n(point, n));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(x: f32, y: f32, shape: u32) -> LaserPoint {
        LaserPoint { x, y, r: 1.0, g: 1.0, b: 1.0, shape }
    }

    /// A square, whose four corners are all right angles.
    fn square() -> LaserFrame {
        LaserFrame {
            points: vec![
                lit(-0.5, -0.5, 0),
                lit(0.5, -0.5, 0),
                lit(0.5, 0.5, 0),
                lit(-0.5, 0.5, 0),
                lit(-0.5, -0.5, 0),
            ],
        }
    }

    #[test]
    fn a_jump_between_strokes_travels_dark() {
        let frame = LaserFrame {
            points: vec![lit(-0.9, 0.0, 0), lit(-0.8, 0.0, 0), lit(0.8, 0.0, 1), lit(0.9, 0.0, 1)],
        };
        let opt = Optimiser { corner_dwell: 0, stroke_repeat: 0, ..Optimiser::default() };

        let out = opt.run(&frame);

        // Every point between the end of stroke 0 and the start of stroke 1
        // must be dark, or the jump draws a line across the room.
        let first_of_second = out
            .points
            .iter()
            .position(|p| p.x > 0.0 && !p.is_blank())
            .expect("second stroke is drawn");
        let last_of_first = out.points.iter().rposition(|p| p.x < 0.0 && !p.is_blank()).unwrap();
        assert!(
            out.points[last_of_first + 1..first_of_second].iter().all(LaserPoint::is_blank),
            "{:?}",
            &out.points[last_of_first + 1..first_of_second]
        );
    }

    #[test]
    fn a_sharp_corner_gets_dwell_and_a_straight_line_does_not() {
        let opt = Optimiser { stroke_repeat: 0, blank_dwell: 0, ..Optimiser::default() };

        let cornered = opt.run(&square()).points.len();
        let straight = opt
            .run(&LaserFrame {
                points: (0..5).map(|i| lit(-0.5 + i as f32 * 0.25, 0.0, 0)).collect(),
            })
            .points
            .len();

        assert_eq!(straight, 5, "a straight line needs no dwell");
        assert!(cornered > 5, "a square's corners need dwell, got {cornered}");
    }

    #[test]
    fn turning_off_angle_optimisation_removes_the_corner_dwell() {
        let settings = rustjay_isf::header::RenderSettings {
            angle_optimization: Some(false),
            ..Default::default()
        };
        let opt = Optimiser {
            stroke_repeat: 0,
            blank_dwell: 0,
            ..Optimiser::from_render_settings(&settings)
        };

        assert_eq!(opt.run(&square()).points.len(), 5);
    }

    #[test]
    fn a_stroke_is_held_at_both_ends() {
        let opt = Optimiser {
            corner_dwell: 0,
            blank_dwell: 0,
            stroke_repeat: 3,
            ..Optimiser::default()
        };

        let out = opt.run(&LaserFrame { points: vec![lit(0.0, 0.0, 0), lit(0.1, 0.0, 0)] });

        // 2 points, plus 3 repeats at each end.
        assert_eq!(out.points.len(), 8);
        assert_eq!(out.points[0], lit(0.0, 0.0, 0));
        assert_eq!(out.points[7], lit(0.1, 0.0, 0));
    }

    #[test]
    fn skip_black_drops_a_stroke_that_emits_nothing() {
        let frame = LaserFrame {
            points: vec![
                LaserPoint::blanked(-0.5, 0.0, 0),
                LaserPoint::blanked(-0.4, 0.0, 0),
                lit(0.4, 0.0, 1),
                lit(0.5, 0.0, 1),
            ],
        };
        let opt = Optimiser { skip_black: true, ..Optimiser::default() };

        let out = opt.run(&frame);

        assert!(out.points.iter().all(|p| p.x > 0.0 || p.is_blank()));
    }

    // Whatever else it does, it must not leave the beam lit and parked.
    #[test]
    fn the_frame_ends_dark() {
        let out = Optimiser::default().run(&square());

        assert!(out.points.last().expect("points").is_blank());
    }

    #[test]
    fn an_empty_frame_stays_empty() {
        assert!(Optimiser::default().run(&LaserFrame::default()).points.is_empty());
    }
}
