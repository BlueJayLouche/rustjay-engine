//! Where the scan field lands in the room.
//!
//! A projector can be aimed by moving the projector. A laser mounted on truss
//! cannot, so the last correction has to happen in the point stream: the
//! material draws in its own square -1..1 field, and this maps that square
//! onto the quad the beam actually needs to cover.
//!
//! The map is projective, not bilinear. Bilinear interpolation of four corners
//! bends straight lines through the interior, and a laser draws almost nothing
//! *but* straight lines — a keystoned square would come out with bowed sides.
//! A homography is also what a 4-point calibration solves, so storing corners
//! and applying them projectively means the correction reproduces exactly what
//! the calibration measured.

use crate::frame::LaserFrame;

/// The identity quad: the whole scan field, corners in TL, TR, BR, BL order.
pub const FULL_FIELD: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];

/// A corner-pin over the scan field.
///
/// Corners are in scan-field units (-1..1), ordered TL, TR, BR, BL to match the
/// warp corners everywhere else in the engine. Keeping them inside the field is
/// what guarantees the mapped path stays inside it: a projective map of the
/// square onto a convex quad lands entirely within that quad.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub corners: [[f32; 2]; 4],
}

impl Default for Geometry {
    fn default() -> Self {
        Self { corners: FULL_FIELD }
    }
}

impl Geometry {
    /// The whole field, uncorrected.
    pub fn identity() -> Self {
        Self::default()
    }

    /// Corners clamped into the field, so a mapped point can never command the
    /// galvos past their travel.
    pub fn with_corners(corners: [[f32; 2]; 4]) -> Self {
        let mut g = Self { corners };
        for c in g.corners.iter_mut() {
            c[0] = c[0].clamp(-1.0, 1.0);
            c[1] = c[1].clamp(-1.0, 1.0);
        }
        g
    }

    /// Whether this is the full field, in which case applying it is a no-op.
    pub fn is_identity(&self) -> bool {
        self.corners == FULL_FIELD
    }

    /// Map one point from the material's field onto the corrected quad.
    pub fn map(&self, x: f32, y: f32) -> (f32, f32) {
        Homography::unit_square_to(&self.corners).map((x + 1.0) * 0.5, (y + 1.0) * 0.5)
    }

    /// Correct a whole frame in place.
    ///
    /// The homography is solved once for the frame rather than per point: it
    /// depends only on the corners, and a frame is thousands of points.
    pub fn apply(&self, frame: &mut LaserFrame) {
        if self.is_identity() {
            return;
        }
        let h = Homography::unit_square_to(&self.corners);
        for p in frame.points.iter_mut() {
            let (x, y) = h.map((p.x + 1.0) * 0.5, (p.y + 1.0) * 0.5);
            p.x = x.clamp(-1.0, 1.0);
            p.y = y.clamp(-1.0, 1.0);
        }
    }
}

/// A projective map, as the 3x3 matrix
///
/// ```text
/// [a b c]
/// [d e f]
/// [g h i]
/// ```
///
/// applied to `(u, v, 1)` and divided through by the result's third component.
/// Kept as a full matrix rather than the eight coefficients of the square-to-
/// quad form because calibration needs to run one *backwards*: it observes
/// where the field's corners landed and must ask which field point would land
/// on a corner it wants.
#[derive(Clone, Copy, Debug)]
pub struct Homography {
    pub m: [f32; 9],
}

impl Homography {
    pub const IDENTITY: Self = Self { m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] };

    /// Solve for the map taking (0,0), (1,0), (1,1), (0,1) to `q`.
    ///
    /// Heckbert's closed form for the square-to-quad case, which needs no
    /// matrix inversion: the source corners are known, so the coefficients fall
    /// out of the destination corners directly.
    pub fn unit_square_to(q: &[[f32; 2]; 4]) -> Self {
        let (x0, y0) = (q[0][0], q[0][1]);
        let (x1, y1) = (q[1][0], q[1][1]);
        let (x2, y2) = (q[2][0], q[2][1]);
        let (x3, y3) = (q[3][0], q[3][1]);

        let sx = x0 - x1 + x2 - x3;
        let sy = y0 - y1 + y2 - y3;

        // A parallelogram closes exactly, and its map is affine — the
        // projective terms are zero and the general form would divide by one.
        if sx.abs() < f32::EPSILON && sy.abs() < f32::EPSILON {
            return Self {
                m: [x1 - x0, x3 - x0, x0, y1 - y0, y3 - y0, y0, 0.0, 0.0, 1.0],
            };
        }

        let dx1 = x1 - x2;
        let dx2 = x3 - x2;
        let dy1 = y1 - y2;
        let dy2 = y3 - y2;
        let den = dx1 * dy2 - dx2 * dy1;
        // Degenerate quad (three corners collinear): no map exists, so fall
        // back to identity rather than emitting infinities at a galvo.
        if den.abs() < f32::EPSILON {
            return Self::IDENTITY;
        }

        let g = (sx * dy2 - dx2 * sy) / den;
        let h = (dx1 * sy - sx * dy1) / den;
        Self {
            m: [
                x1 - x0 + g * x1,
                x3 - x0 + h * x3,
                x0,
                y1 - y0 + g * y1,
                y3 - y0 + h * y3,
                y0,
                g,
                h,
                1.0,
            ],
        }
    }

    /// The reverse map.
    ///
    /// A projective matrix is defined up to scale, so the adjugate serves as an
    /// inverse with no determinant division.
    pub fn inverse(&self) -> Self {
        let [a, b, c, d, e, f, g, h, i] = self.m;
        Self {
            m: [
                e * i - f * h,
                c * h - b * i,
                b * f - c * e,
                f * g - d * i,
                a * i - c * g,
                c * d - a * f,
                d * h - e * g,
                b * g - a * h,
                a * e - b * d,
            ],
        }
    }

    /// The map that applies `self` and then `next`.
    pub fn then(&self, next: &Self) -> Self {
        let (a, b) = (&next.m, &self.m);
        let mut m = [0.0; 9];
        for row in 0..3 {
            for col in 0..3 {
                m[row * 3 + col] = (0..3).map(|k| a[row * 3 + k] * b[k * 3 + col]).sum();
            }
        }
        Self { m }
    }

    /// The map taking one arbitrary quad onto another.
    ///
    /// Routed through the unit square in both directions, because that is the
    /// only case with a closed form.
    pub fn quad_to_quad(src: &[[f32; 2]; 4], dst: &[[f32; 2]; 4]) -> Self {
        Self::unit_square_to(src).inverse().then(&Self::unit_square_to(dst))
    }

    pub fn map(&self, u: f32, v: f32) -> (f32, f32) {
        let [a, b, c, d, e, f, g, h, i] = self.m;
        let w = g * u + h * v + i;
        // A point on the map's horizon has no finite image. Passing it through
        // unchanged keeps a bad calibration from turning into an infinity.
        if w.abs() < f32::EPSILON {
            return (u, v);
        }
        ((a * u + b * v + c) / w, (d * u + e * v + f) / w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::LaserPoint;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// The full field maps every point to itself, and short-circuits.
    #[test]
    fn identity_leaves_a_frame_alone() {
        let g = Geometry::identity();
        assert!(g.is_identity());

        let mut frame = LaserFrame {
            points: vec![
                LaserPoint { x: -0.5, y: 0.25, r: 1.0, g: 1.0, b: 1.0, shape: 0 },
                LaserPoint { x: 0.75, y: -1.0, r: 1.0, g: 1.0, b: 1.0, shape: 0 },
            ],
        };
        let before = frame.clone();
        g.apply(&mut frame);
        assert_eq!(frame, before);
    }

    /// The corners of the field land exactly on the corners of the quad —
    /// which is the property a 4-point calibration relies on.
    #[test]
    fn field_corners_land_on_quad_corners() {
        let quad = [[-0.5, -0.9], [0.6, -0.7], [1.0, 0.8], [-1.0, 0.6]];
        let g = Geometry::with_corners(quad);

        for (i, (fx, fy)) in
            [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].into_iter().enumerate()
        {
            let (x, y) = g.map(fx, fy);
            assert!(
                close(x, quad[i][0]) && close(y, quad[i][1]),
                "corner {i}: got ({x}, {y}), want ({}, {})",
                quad[i][0],
                quad[i][1]
            );
        }
    }

    /// The map is projective, so a straight line stays straight. Bilinear
    /// interpolation is what this test exists to rule out: on a keystone it
    /// bows the mid-edge point away from the true edge midpoint.
    #[test]
    fn a_keystone_keeps_straight_lines_straight() {
        // Narrow at the top, wide at the bottom.
        let g = Geometry::with_corners([[-0.5, -1.0], [0.5, -1.0], [1.0, 1.0], [-1.0, 1.0]]);

        // Three collinear points down the left edge of the field.
        let (ax, ay) = g.map(-1.0, -1.0);
        let (bx, by) = g.map(-1.0, 0.0);
        let (cx, cy) = g.map(-1.0, 1.0);

        // Cross product of (b-a) and (c-a) is zero when the three are collinear.
        let cross = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
        assert!(cross.abs() < 1e-4, "left edge bowed: cross = {cross}");

        // And the midpoint is *not* the average of the ends, which is exactly
        // where a bilinear map would have put it.
        assert!(!close(bx, (ax + cx) * 0.5), "map looks bilinear, not projective");
    }

    /// The reverse map undoes the forward one, which is what lets calibration
    /// ask "which field point lands *there*".
    #[test]
    fn the_inverse_undoes_the_map() {
        let h = Homography::unit_square_to(&[[-0.5, -0.9], [0.6, -0.7], [1.0, 0.8], [-1.0, 0.6]]);
        let inv = h.inverse();

        for (u, v) in [(0.0, 0.0), (1.0, 0.0), (0.25, 0.75), (0.5, 0.5)] {
            let (x, y) = h.map(u, v);
            let (bu, bv) = inv.map(x, y);
            assert!(close(bu, u) && close(bv, v), "({u},{v}) -> ({x},{y}) -> ({bu},{bv})");
        }
    }

    /// Composition is what carries a quad onto a quad via the unit square.
    #[test]
    fn quad_to_quad_takes_corners_to_corners() {
        let src = [[-1.0, -1.0], [0.5, -0.8], [0.9, 1.0], [-0.7, 0.6]];
        let dst = [[0.0, 0.0], [100.0, 10.0], [90.0, 80.0], [5.0, 70.0]];
        let h = Homography::quad_to_quad(&src, &dst);

        for i in 0..4 {
            let (x, y) = h.map(src[i][0], src[i][1]);
            assert!(
                (x - dst[i][0]).abs() < 1e-2 && (y - dst[i][1]).abs() < 1e-2,
                "corner {i}: got ({x}, {y}), want {:?}",
                dst[i]
            );
        }
    }

    /// Corners outside the field are pulled back to it, so no mapped point can
    /// command the galvos past their travel.
    #[test]
    fn corners_are_clamped_into_the_field() {
        let g = Geometry::with_corners([[-3.0, -2.0], [1.5, -1.0], [1.0, 1.0], [-1.0, 4.0]]);
        for c in g.corners {
            assert!((-1.0..=1.0).contains(&c[0]), "x out of field: {}", c[0]);
            assert!((-1.0..=1.0).contains(&c[1]), "y out of field: {}", c[1]);
        }
    }
}
