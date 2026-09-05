//! The point list a laser material produced, decoded from the texture it drew
//! into.
//!
//! MadMapper documents that texture's layout and the shader bridge in
//! `rustjay-isf` writes it: `POINT_COUNT` wide, one row per field.
//!
//! ```text
//! row 0:  r,g = position in -1..1     b = shape number      a = unused
//! row 1:  r,g,b = colour in 0..1      a = unused
//! row 2:  user data, carried to the next frame
//! ```
//!
//! Row 2 never reaches here: it goes straight back to the GPU as the next
//! frame's `mm_LastFrameData`, so only the first two rows are read back.

/// Rows that have to reach the CPU — position and colour. Row 2 is feedback
/// and stays on the GPU.
pub const READBACK_ROWS: u32 = 2;

/// Bytes per texel of the target's `Rgba32Float`.
pub const TEXEL_BYTES: usize = 16;

/// One sample on a 2D path.
///
/// A run of points sharing a `shape` is one continuous stroke; the beam jumps
/// (blanked) wherever `shape` changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaserPoint {
    /// Position in -1..1, origin centre.
    pub x: f32,
    pub y: f32,
    /// Colour in 0..1. Alpha is meaningless for a laser — there is nothing to
    /// composite against — so it is not carried.
    pub r: f32,
    pub g: f32,
    pub b: f32,
    /// Which stroke this point belongs to.
    pub shape: u32,
}

impl LaserPoint {
    /// A point the beam passes through with the light off.
    pub fn blanked(x: f32, y: f32, shape: u32) -> Self {
        Self { x, y, r: 0.0, g: 0.0, b: 0.0, shape }
    }

    /// True when this point emits no light, so the beam is travelling rather
    /// than drawing.
    pub fn is_blank(&self) -> bool {
        self.r <= 0.0 && self.g <= 0.0 && self.b <= 0.0
    }

    /// Distance to another point in the -1..1 scan field.
    pub fn distance_to(&self, other: &Self) -> f32 {
        (other.x - self.x).hypot(other.y - self.y)
    }
}

/// One frame's worth of path, in the order the material generated it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaserFrame {
    pub points: Vec<LaserPoint>,
}

impl LaserFrame {
    /// Decode a readback of the first [`READBACK_ROWS`] rows.
    ///
    /// `row_pitch` is the buffer's stride in bytes, which wgpu pads to a
    /// 256-byte multiple and is therefore usually wider than the texture.
    /// A buffer too short to hold both rows yields no points at all.
    pub fn decode(bytes: &[u8], point_count: usize, row_pitch: usize) -> Self {
        // Both rows have to be there. Half a readback would have colours read
        // as coordinates, so a short buffer yields nothing at all — which the
        // scan-fail guard then blanks, rather than aiming at whatever it found.
        if row_pitch < TEXEL_BYTES || bytes.len() < READBACK_ROWS as usize * row_pitch {
            return Self::default();
        }
        let texel = |row: usize, i: usize| -> [f32; 4] {
            let at = row * row_pitch + i * TEXEL_BYTES;
            let mut out = [0.0; 4];
            let (words, _) = bytes[at..at + TEXEL_BYTES].as_chunks::<4>();
            for (o, b) in out.iter_mut().zip(words) {
                *o = f32::from_le_bytes(*b);
            }
            out
        };

        // A row holds this many texels including wgpu's stride padding; the
        // point count should never exceed it, but a caller need not prove that.
        let count = point_count.min(row_pitch / TEXEL_BYTES);
        let mut points = Vec::with_capacity(count);
        for i in 0..count {
            let (pos, col) = (texel(0, i), texel(1, i));
            points.push(LaserPoint {
                x: in_field(pos[0]),
                y: in_field(pos[1]),
                r: unit(col[0]),
                g: unit(col[1]),
                b: unit(col[2]),
                // The shape number rides in a colour channel but is not one:
                // it counts strokes, so it is not clamped to a colour's range.
                shape: if pos[2].is_finite() { pos[2].max(0.0).round() as u32 } else { 0 },
            });
        }
        Self { points }
    }

    /// Total distance the beam travels over the frame.
    ///
    /// The scan-fail guard reads this: a frame whose beam barely moves is a
    /// stationary dot, which is the dangerous failure a shader bug produces.
    pub fn path_length(&self) -> f32 {
        self.points
            .windows(2)
            .map(|w| w[0].distance_to(&w[1]))
            .sum()
    }

    /// The largest dimension of the box containing every point that emits
    /// light, in scan-field units where the whole field is 2 across — or
    /// `None` when the frame emits no light at all.
    ///
    /// This is what the scan-fail guard reads. Path length would miss a beam
    /// scribbling tightly in one place, which concentrates just as much energy
    /// on one spot as a stationary dot does. Blanked points are ignored: the
    /// beam is off, so where it travels is not a hazard — and a frame that is
    /// entirely blanked is the safest frame there is, which is why that case
    /// is `None` rather than an extent of zero.
    pub fn lit_extent(&self) -> Option<f32> {
        let mut lit = self.points.iter().filter(|p| !p.is_blank());
        let first = lit.next()?;
        let (mut min, mut max) = ((first.x, first.y), (first.x, first.y));
        for p in lit {
            min = (min.0.min(p.x), min.1.min(p.y));
            max = (max.0.max(p.x), max.1.max(p.y));
        }
        Some((max.0 - min.0).max(max.1 - min.1))
    }

    /// Index ranges of each stroke, split where the shape number changes.
    pub fn strokes(&self) -> Vec<std::ops::Range<usize>> {
        let mut out: Vec<std::ops::Range<usize>> = Vec::new();
        for (i, p) in self.points.iter().enumerate() {
            match out.last_mut() {
                Some(last) if self.points[last.start].shape == p.shape => last.end = i + 1,
                _ => out.push(i..i + 1),
            }
        }
        out
    }
}

/// A coordinate in the scan field. A shader can emit NaN, infinity or a wild
/// magnitude; none of those belong in a galvo command.
fn in_field(v: f32) -> f32 {
    if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 }
}

/// A colour channel, which is off rather than undefined when the shader
/// produced nonsense.
fn unit(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a readback buffer holding `rows` of `texels`, padded to `pitch`.
    fn buffer(rows: &[Vec<[f32; 4]>], pitch: usize) -> Vec<u8> {
        let mut out = vec![0u8; rows.len() * pitch];
        for (r, row) in rows.iter().enumerate() {
            for (i, t) in row.iter().enumerate() {
                let at = r * pitch + i * TEXEL_BYTES;
                for (c, v) in t.iter().enumerate() {
                    out[at + c * 4..at + c * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
        }
        out
    }

    #[test]
    fn a_frame_decodes_position_colour_and_shape() {
        let bytes = buffer(
            &[
                vec![[-1.0, 0.5, 0.0, 0.0], [1.0, -0.5, 2.0, 0.0]],
                vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0]],
            ],
            256,
        );

        let frame = LaserFrame::decode(&bytes, 2, 256);

        assert_eq!(frame.points.len(), 2);
        assert_eq!(frame.points[0], LaserPoint { x: -1.0, y: 0.5, r: 1.0, g: 0.0, b: 0.0, shape: 0 });
        assert_eq!(frame.points[1].shape, 2);
    }

    // Positions come from a shader; a divide by zero must not reach a galvo.
    #[test]
    fn a_non_finite_position_becomes_zero() {
        let bytes = buffer(
            &[vec![[f32::NAN, f32::INFINITY, 0.0, 0.0]], vec![[1.0, 1.0, 1.0, 1.0]]],
            256,
        );

        let frame = LaserFrame::decode(&bytes, 1, 256);

        assert_eq!((frame.points[0].x, frame.points[0].y), (0.0, 0.0));
    }

    #[test]
    fn an_out_of_range_position_is_clamped_to_the_scan_field() {
        let bytes = buffer(&[vec![[-40.0, 40.0, 0.0, 0.0]], vec![[1.0, 1.0, 1.0, 1.0]]], 256);

        let frame = LaserFrame::decode(&bytes, 1, 256);

        assert_eq!((frame.points[0].x, frame.points[0].y), (-1.0, 1.0));
    }

    // Asking for more points than the row holds gets the row, not a read into
    // the next one.
    #[test]
    fn a_point_count_past_the_row_stops_at_the_row() {
        let bytes = buffer(&[vec![[0.5; 4]; 16], vec![[1.0; 4]; 16]], 256);

        let frame = LaserFrame::decode(&bytes, 99, 256);

        assert_eq!(frame.points.len(), 256 / TEXEL_BYTES);
    }

    // Half a readback would put colours where coordinates go.
    #[test]
    fn a_truncated_readback_yields_no_points() {
        let bytes = buffer(&[vec![[0.5; 4]]], 256);

        assert!(LaserFrame::decode(&bytes, 8, 256).points.is_empty());
        assert!(LaserFrame::decode(&[], 8, 256).points.is_empty());
    }

    #[test]
    fn a_shape_number_is_not_clamped_like_a_colour() {
        let bytes = buffer(
            &[vec![[0.0, 0.0, 7.0, 0.0]], vec![[2.0, -1.0, 0.5, 1.0]]],
            256,
        );

        let frame = LaserFrame::decode(&bytes, 1, 256);

        assert_eq!(frame.points[0].shape, 7);
        // ...while the colour still is.
        assert_eq!((frame.points[0].r, frame.points[0].g), (1.0, 0.0));
    }

    #[test]
    fn strokes_split_where_the_shape_number_changes() {
        let frame = LaserFrame {
            points: vec![
                LaserPoint::blanked(0.0, 0.0, 0),
                LaserPoint::blanked(0.1, 0.0, 0),
                LaserPoint::blanked(0.5, 0.0, 1),
                LaserPoint::blanked(0.6, 0.0, 1),
                LaserPoint::blanked(0.9, 0.0, 0),
            ],
        };

        // The last point returns to shape 0 but is a new stroke, not a
        // continuation of the first — shapes group by run, not by number.
        assert_eq!(frame.strokes(), vec![0..2, 2..4, 4..5]);
    }

    #[test]
    fn path_length_is_zero_for_a_stationary_beam() {
        let frame = LaserFrame {
            points: vec![LaserPoint::blanked(0.2, 0.2, 0); 64],
        };

        assert_eq!(frame.path_length(), 0.0);
    }
}
