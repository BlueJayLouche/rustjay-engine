//! Learning where the beam actually lands, with a camera.
//!
//! The procedure is the same shape as the LED one in `rustjay-ledmap`: show a
//! target, find it in a webcam frame, repeat, then solve. What is *not* the
//! same is the target.
//!
//! An LED calibration lights one LED and looks for the bright spot. The laser
//! equivalent — park the beam at a corner and look for the dot — is precisely
//! what [`crate::safety`] exists to prevent: a stationary beam puts the whole
//! output into one spot with no scanning to spread it, and [`Blocked::ScanFail`]
//! blocks any frame whose lit points cover almost no area. That gate is not
//! negotiable and is never relaxed for calibration.
//!
//! So a target here is a small traced **ring**, not a dot. It clears
//! [`crate::safety::MIN_LIT_EXTENT`] by more than a decade, keeps the beam
//! moving the whole time, and still gives the camera a blob whose centroid is
//! the point we want to measure.
//!
//! [`Blocked::ScanFail`]: crate::safety::Blocked::ScanFail

use crate::frame::{LaserFrame, LaserPoint};
use crate::geometry::{Geometry, Homography};

/// Radius of a calibration ring, in scan-field units.
///
/// The field is 2 across, so this is a ring 6% of it wide — an order of
/// magnitude above the scan-fail floor, and small enough that its centroid
/// locates a corner to well under a percent of the field.
pub const TARGET_RADIUS: f32 = 0.06;

/// Samples around one ring. Enough that the ring reads as a circle to the
/// camera rather than a polygon, at any refresh a scanner can manage.
pub const TARGET_POINTS: usize = 180;

/// Where the four rings are drawn, in TL, TR, BR, BL order.
///
/// Inset by the ring's own radius so the whole ring is inside the field: a ring
/// centred exactly on a corner would have half of it clipped, and the visible
/// half's centroid is not the corner.
pub fn target_centres(radius: f32) -> [[f32; 2]; 4] {
    let k = 1.0 - radius;
    [[-k, -k], [k, -k], [k, k], [-k, k]]
}

/// The frame that draws calibration target `index` (0..4).
///
/// White at full brightness: the camera is looking for the brightest blob, and
/// colour tells it nothing.
pub fn target_frame(index: usize, radius: f32) -> LaserFrame {
    let c = target_centres(radius)[index.min(3)];
    // The loop is closed — the last point repeats the first — so the optimiser
    // sees one continuous stroke rather than a stroke with a gap to jump.
    let points = (0..=TARGET_POINTS)
        .map(|i| {
            let a = i as f32 / TARGET_POINTS as f32 * std::f32::consts::TAU;
            LaserPoint {
                x: c[0] + radius * a.cos(),
                y: c[1] + radius * a.sin(),
                r: 1.0,
                g: 1.0,
                b: 1.0,
                shape: 0,
            }
        })
        .collect();
    LaserFrame { points }
}

/// Solve the correction from what the camera saw.
///
/// `observed` is where the four rings appeared, in whatever coordinates the
/// camera reports (pixels are fine — the solve is scale-free), in the same
/// TL, TR, BR, BL order as [`target_centres`]. `desired` is where, in those
/// same camera coordinates, the corners of the field should end up.
///
/// The result is the corner-pin that gets you there: it asks the reverse of the
/// measured map which field point lands on each corner you asked for.
///
/// Run this with the deck's geometry at identity. The rings are emitted through
/// whatever correction is already applied, so calibrating on top of an existing
/// one measures the pair and solves for the wrong thing.
pub fn solve(observed: &[[f32; 2]; 4], desired: &[[f32; 2]; 4], radius: f32) -> Geometry {
    let field_to_camera = Homography::quad_to_quad(&target_centres(radius), observed);
    let camera_to_field = field_to_camera.inverse();
    let mut corners = [[0.0_f32; 2]; 4];
    for (corner, want) in corners.iter_mut().zip(desired) {
        let (x, y) = camera_to_field.map(want[0], want[1]);
        *corner = [x, y];
    }
    Geometry::with_corners(corners)
}

/// The rectangle in camera coordinates that the observed quad most nearly
/// fills, as a starting point for `desired`.
///
/// Squaring up what the laser already covers is the common case — undo the
/// keystone, keep the reach — and it saves the operator dragging four corners
/// before they have seen anything happen.
pub fn squared_up(observed: &[[f32; 2]; 4]) -> [[f32; 2]; 4] {
    let xs = observed.map(|p| p[0]);
    let ys = observed.map(|p| p[1]);
    // The *inner* bounds, not the bounding box: a rectangle out to the extremes
    // would ask for corners beyond what the scanner reached on at least one
    // side, and the solve would answer with corners outside the field.
    let min_x = xs[0].max(xs[3]);
    let max_x = xs[1].min(xs[2]);
    let min_y = ys[0].max(ys[1]);
    let max_y = ys[2].min(ys[3]);
    [[min_x, min_y], [max_x, min_y], [max_x, max_y], [min_x, max_y]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safety::{MIN_LIT_EXTENT, Safety};

    /// The reason this module draws rings. A parked dot is blocked by the
    /// safety gate — correctly — so a target has to be something that moves.
    #[test]
    fn a_target_passes_the_scan_fail_gate() {
        let frame = target_frame(0, TARGET_RADIUS);
        let extent = frame.lit_extent().expect("target is lit");
        assert!(
            extent > MIN_LIT_EXTENT * 10.0,
            "target extent {extent} is not comfortably above the {MIN_LIT_EXTENT} floor"
        );

        let mut safety = Safety::default();
        safety.arm();
        assert!(safety.gate(&frame, true).is_some(), "a calibration target must not be blocked");
    }

    /// A parked dot, for contrast: still blocked, and this test is here so that
    /// stays true if anyone ever "simplifies" the target back to one.
    #[test]
    fn a_parked_dot_is_still_blocked() {
        let dot = LaserFrame {
            points: vec![LaserPoint { x: 0.5, y: 0.5, r: 1.0, g: 1.0, b: 1.0, shape: 0 }; 64],
        };
        let mut safety = Safety::default();
        safety.arm();
        assert!(safety.gate(&dot, true).is_none(), "a parked beam must never be sent");
    }

    /// Every ring sits wholly inside the field, so the camera sees a full
    /// circle whose centroid is the point being measured.
    #[test]
    fn targets_fit_inside_the_field() {
        for i in 0..4 {
            for p in target_frame(i, TARGET_RADIUS).points {
                assert!(p.x >= -1.001 && p.x <= 1.001, "target {i} x escaped: {}", p.x);
                assert!(p.y >= -1.001 && p.y <= 1.001, "target {i} y escaped: {}", p.y);
            }
        }
    }

    /// End to end against a simulated rig: a projector with a known keystone,
    /// a camera that sees it, and the correction that squares it up.
    #[test]
    fn solving_makes_the_field_land_where_it_was_asked_to() {
        // The rig: whatever the deck emits in field units lands here, in camera
        // pixels. Steeply keystoned and rotated, as a laser on truss would be.
        let rig = Homography::quad_to_quad(
            &crate::geometry::FULL_FIELD,
            &[[120.0, 60.0], [880.0, 20.0], [960.0, 700.0], [40.0, 620.0]],
        );
        let project = |p: [f32; 2]| {
            let (x, y) = rig.map(p[0], p[1]);
            [x, y]
        };

        // Step 1: show the four rings, see where they land.
        let observed = target_centres(TARGET_RADIUS).map(project);

        // Step 2: ask for a tidy rectangle in the same camera coordinates.
        let desired = [[200.0, 150.0], [800.0, 150.0], [800.0, 550.0], [200.0, 550.0]];

        // Step 3: solve.
        let g = solve(&observed, &desired, TARGET_RADIUS);

        // Now emitting the field's corners through the correction should land
        // on the rectangle that was asked for.
        for i in 0..4 {
            let landed = project(g.corners[i]);
            assert!(
                (landed[0] - desired[i][0]).abs() < 1.0
                    && (landed[1] - desired[i][1]).abs() < 1.0,
                "corner {i} landed at {landed:?}, wanted {:?}",
                desired[i]
            );
        }
    }

    /// A rig that is already square needs no correction worth speaking of.
    #[test]
    fn a_square_rig_solves_to_near_identity() {
        let rig = Homography::quad_to_quad(
            &crate::geometry::FULL_FIELD,
            &[[0.0, 0.0], [1000.0, 0.0], [1000.0, 1000.0], [0.0, 1000.0]],
        );
        let project = |p: [f32; 2]| {
            let (x, y) = rig.map(p[0], p[1]);
            [x, y]
        };
        let observed = target_centres(TARGET_RADIUS).map(project);
        let desired = crate::geometry::FULL_FIELD.map(project);

        let g = solve(&observed, &desired, TARGET_RADIUS);
        for (got, want) in g.corners.iter().zip(crate::geometry::FULL_FIELD) {
            assert!(
                (got[0] - want[0]).abs() < 1e-3 && (got[1] - want[1]).abs() < 1e-3,
                "got {got:?}, want {want:?}"
            );
        }
    }

    /// Squaring up stays within what the scanner actually reached, so the
    /// solve cannot answer with corners outside the field.
    #[test]
    fn squared_up_stays_inside_the_observed_quad() {
        let observed = [[120.0, 60.0], [880.0, 20.0], [960.0, 700.0], [40.0, 620.0]];
        let r = squared_up(&observed);

        assert_eq!(r[0][0], 120.0, "left edge should be the inner of the two lefts");
        assert_eq!(r[1][0], 880.0, "right edge should be the inner of the two rights");
        assert_eq!(r[0][1], 60.0, "top edge should be the inner of the two tops");
        assert_eq!(r[2][1], 620.0, "bottom edge should be the inner of the two bottoms");
    }
}
