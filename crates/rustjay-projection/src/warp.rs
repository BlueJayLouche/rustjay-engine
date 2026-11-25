//! Warp pipeline — perspective correction and UV mesh warping for projection mapping.

use crate::stage::ProjectionStage;
use rustjay_core::RenderCtx;
use wgpu::util::DeviceExt;

// ── Mesh types ───────────────────────────────────────────────────────────

/// A single point in a UV warp mesh: output-space position + source-space UV.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MeshPoint {
    /// Position in output-normalized coords [0..1]
    pub position: [f32; 2],
    /// UV coordinates in source texture space [0..1]
    pub uv: [f32; 2],
}

/// A grid of XYUV warp points defining an arbitrary mesh warp.
///
/// Points are stored row-major: `points[row * cols + col]`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WarpMesh {
    /// Number of columns in the grid (≥2)
    pub cols: u32,
    /// Number of rows in the grid (≥2)
    pub rows: u32,
    /// Grid points, row-major order. Length = cols × rows.
    pub points: Vec<MeshPoint>,
}

impl WarpMesh {
    pub fn identity(cols: u32, rows: u32) -> Self {
        let mut points = Vec::with_capacity((cols * rows) as usize);
        for r in 0..rows {
            let v = r as f32 / (rows - 1).max(1) as f32;
            for c in 0..cols {
                let u = c as f32 / (cols - 1).max(1) as f32;
                points.push(MeshPoint {
                    position: [u, v],
                    uv: [u, v],
                });
            }
        }
        Self { cols, rows, points }
    }

    /// Create a mesh from 4 corner positions (corner-pin equivalent).
    /// Order: TL, TR, BR, BL → grid row-major: TL, TR, BL, BR
    pub fn from_corners(corners: &[[f32; 2]; 4]) -> Self {
        Self {
            cols: 2,
            rows: 2,
            points: vec![
                MeshPoint {
                    position: corners[0],
                    uv: [0.0, 0.0],
                }, // TL
                MeshPoint {
                    position: corners[1],
                    uv: [1.0, 0.0],
                }, // TR
                MeshPoint {
                    position: corners[3],
                    uv: [0.0, 1.0],
                }, // BL
                MeshPoint {
                    position: corners[2],
                    uv: [1.0, 1.0],
                }, // BR
            ],
        }
    }

    pub fn is_identity(&self) -> bool {
        self.points.iter().all(|p| {
            (p.position[0] - p.uv[0]).abs() < 1e-6 && (p.position[1] - p.uv[1]).abs() < 1e-6
        })
    }
}

/// Warp mode for surface assignments: corner-pin or arbitrary mesh.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum WarpMode {
    /// 4-point corner-pin warp (TL, TR, BR, BL in output space [0..1]).
    CornerPin {
        /// The four corner positions in normalized output coordinates.
        corners: [[f32; 2]; 4],
    },
    /// Arbitrary XYUV mesh warp grid.
    Mesh(WarpMesh),
    /// Bezier patch grid, tessellated into a [`WarpMesh`] for rendering.
    Bezier(BezierWarp),
}

impl WarpMode {
    pub fn corner_pin(corners: [[f32; 2]; 4]) -> Self {
        Self::CornerPin { corners }
    }

    pub fn identity() -> Self {
        Self::CornerPin {
            corners: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        }
    }

    pub fn is_identity(&self) -> bool {
        match self {
            Self::CornerPin { corners } => {
                let id = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
                corners
                    .iter()
                    .zip(id.iter())
                    .all(|(a, b)| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6)
            }
            Self::Mesh(mesh) => mesh.is_identity(),
            Self::Bezier(b) => b.is_identity(),
        }
    }
}

// ── Bezier patch-grid warp ───────────────────────────────────────────────
//
// Ported from varda (`src/internal/surface/warp.rs`, MIT); same JSON shape, so
// warps round-trip between the two. An `anchor_cols × anchor_rows` cage of
// anchors with cubic tangent handles on each edge; each cell is a Coons patch
// bounded by four cubics. Added here: `perspective`, which evaluates the cage
// in the projective frame of its four outer corners so a straight-edged cage
// is exactly the `CornerPin` homography instead of a bilinear quad.

/// Maximum tessellated grid resolution (columns or rows) of a bezier warp.
pub const MAX_WARP_SUBDIVISIONS: u32 = 64;

/// Default tessellation steps per patch edge for a new bezier warp.
pub const DEFAULT_BEZIER_TESS: u32 = 6;

const UNIT_SQUARE: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

/// A smooth warp defined by a grid of cubic-bezier patches with tangent handles.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BezierWarp {
    /// Anchor columns in the control cage (≥2).
    pub anchor_cols: u32,
    /// Anchor rows in the control cage (≥2).
    pub anchor_rows: u32,
    /// Anchor positions, row-major, output space [0..1].
    pub anchors: Vec<[f32; 2]>,
    /// Horizontal-edge handles `[near-left, near-right]`, row-major over rows
    /// then per-row edges. Length `anchor_rows · (anchor_cols − 1)`.
    pub h_horiz: Vec<[[f32; 2]; 2]>,
    /// Vertical-edge handles `[near-top, near-bottom]`, row-major over
    /// edge-rows then cols. Length `anchor_cols · (anchor_rows − 1)`.
    pub h_vert: Vec<[[f32; 2]; 2]>,
    /// Tessellation steps per patch edge (≥1). Controls output mesh density.
    pub tess: u32,
    /// Evaluate in the perspective frame of the four outer corners (exact
    /// corner-pin when edges are straight). Absent in varda files → bilinear.
    #[serde(default)]
    pub perspective: bool,
    /// Source-space `u` (increasing, 0 → 1) of each anchor column; empty means
    /// evenly spaced. Set by row/column insert and delete so the video mapping
    /// doesn't shift.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub col_u: Vec<f32>,
    /// Source-space `v` of each anchor row, like [`Self::col_u`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub row_v: Vec<f32>,
}

type Edge = [[f32; 2]; 2];

fn add2(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub2(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn scale2(a: [f32; 2], k: f32) -> [f32; 2] {
    [a[0] * k, a[1] * k]
}

fn lerp2(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Apply a row-major 3×3 homography to a point.
pub fn apply_homography(h: &[f32; 9], p: [f32; 2]) -> [f32; 2] {
    let w = h[6] * p[0] + h[7] * p[1] + h[8];
    [
        (h[0] * p[0] + h[1] * p[1] + h[2]) / w,
        (h[3] * p[0] + h[4] * p[1] + h[5]) / w,
    ]
}

fn cubic_point(p0: [f32; 2], c1: [f32; 2], c2: [f32; 2], p1: [f32; 2], t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [
        a * p0[0] + b * c1[0] + c * c2[0] + d * p1[0],
        a * p0[1] + b * c1[1] + c * c2[1] + d * p1[1],
    ]
}

/// Split a cubic at `t` (de Casteljau): left handles, split point, right handles.
fn split_cubic(p0: [f32; 2], c1: [f32; 2], c2: [f32; 2], p3: [f32; 2], t: f32) -> (Edge, [f32; 2], Edge) {
    let (a, b, c) = (lerp2(p0, c1, t), lerp2(c1, c2, t), lerp2(c2, p3, t));
    let (d, e) = (lerp2(a, b, t), lerp2(b, c, t));
    ([a, d], lerp2(d, e, t), [e, c])
}

/// Transfinite (Coons) blend of a patch's four boundary points at `(s, t)`.
#[allow(clippy::too_many_arguments, clippy::many_single_char_names)]
fn coons_blend(
    l: [f32; 2],
    r: [f32; 2],
    tp: [f32; 2],
    bt: [f32; 2],
    c00: [f32; 2],
    c10: [f32; 2],
    c01: [f32; 2],
    c11: [f32; 2],
    s: f32,
    t: f32,
) -> [f32; 2] {
    let mut pos = [0.0f32; 2];
    for d in 0..2 {
        let lc = (1.0 - s) * l[d] + s * r[d];
        let ld = (1.0 - t) * tp[d] + t * bt[d];
        let b = (1.0 - s) * (1.0 - t) * c00[d]
            + s * (1.0 - t) * c10[d]
            + (1.0 - s) * t * c01[d]
            + s * t * c11[d];
        pos[d] = lc + ld - b;
    }
    pos
}

/// Parameter of grid line `i` of `n`: from `knots`, or evenly spaced when empty.
fn knot(knots: &[f32], i: usize, n: usize) -> f32 {
    if knots.len() == n {
        knots[i]
    } else {
        i as f32 / (n - 1) as f32
    }
}

/// The patch containing global parameter `x`, and `x`'s local parameter in it.
fn locate(knots: &[f32], n: usize, x: f32) -> (usize, f32) {
    let mut i = 0;
    while i + 2 < n && knot(knots, i + 1, n) <= x {
        i += 1;
    }
    let (a, b) = (knot(knots, i, n), knot(knots, i + 1, n));
    (i, (x - a) / (b - a))
}

/// `(forward, inverse)` homography between the unit square and a cage frame.
type Frame = ([f32; 9], [f32; 9]);

impl BezierWarp {
    /// Straight-edge handles between `a` and `b`: the ⅓/⅔ chord points, taken
    /// in the perspective frame when there is one.
    fn straight(a: [f32; 2], b: [f32; 2], frame: Option<&Frame>) -> Edge {
        match frame {
            None => [lerp2(a, b, 1.0 / 3.0), lerp2(a, b, 2.0 / 3.0)],
            Some((h, inv)) => {
                let (ua, ub) = (apply_homography(inv, a), apply_homography(inv, b));
                [
                    apply_homography(h, lerp2(ua, ub, 1.0 / 3.0)),
                    apply_homography(h, lerp2(ua, ub, 2.0 / 3.0)),
                ]
            }
        }
    }

    /// Build a bezier warp from an anchor grid with all edges straight. Anchor
    /// count must equal `anchor_cols · anchor_rows` (padded if short). `tess` ≥1.
    pub fn from_anchors(
        anchor_cols: u32,
        anchor_rows: u32,
        mut anchors: Vec<[f32; 2]>,
        tess: u32,
        perspective: bool,
    ) -> Self {
        let ac = anchor_cols.clamp(2, MAX_WARP_SUBDIVISIONS);
        let ar = anchor_rows.clamp(2, MAX_WARP_SUBDIVISIONS);
        anchors.resize((ac * ar) as usize, [0.0, 0.0]);
        let mut w = Self {
            anchor_cols: ac,
            anchor_rows: ar,
            anchors,
            h_horiz: Vec::new(),
            h_vert: Vec::new(),
            tess: tess.max(1),
            perspective,
            col_u: Vec::new(),
            row_v: Vec::new(),
        };
        let frame = w.frame();
        let aci = ac as usize;
        for r in 0..ar as usize {
            for c in 0..aci - 1 {
                let h = Self::straight(w.anchor(r, c), w.anchor(r, c + 1), frame.as_ref());
                w.h_horiz.push(h);
            }
        }
        for r in 0..ar as usize - 1 {
            for c in 0..aci {
                let h = Self::straight(w.anchor(r, c), w.anchor(r + 1, c), frame.as_ref());
                w.h_vert.push(h);
            }
        }
        w
    }

    /// A 2×2 perspective cage on `corners` (TL, TR, BR, BL) with straight
    /// edges — an exact corner-pin that can then be curved.
    pub fn corner_pin(corners: [[f32; 2]; 4], tess: u32) -> Self {
        let anchors = vec![corners[0], corners[1], corners[3], corners[2]];
        Self::from_anchors(2, 2, anchors, tess, true)
    }

    /// Seed a bezier warp from a mesh: anchors are the mesh points, edges
    /// straight, bilinear. At `tess = 1` it tessellates back to the same mesh.
    pub fn from_mesh(mesh: &WarpMesh, tess: u32) -> Self {
        let anchors = mesh.points.iter().map(|p| p.position).collect();
        Self::from_anchors(mesh.cols, mesh.rows, anchors, tess, false)
    }

    /// Check array lengths and bounds. Call on anything loaded from a file:
    /// the other methods index without checking.
    pub fn validate(&self) -> anyhow::Result<()> {
        let (ac, ar) = (self.anchor_cols as usize, self.anchor_rows as usize);
        let max = MAX_WARP_SUBDIVISIONS as usize;
        if !(2..=max).contains(&ac) || !(2..=max).contains(&ar) {
            anyhow::bail!("bezier warp: cage must be 2..={max} anchors per side");
        }
        if self.anchors.len() != ac * ar
            || self.h_horiz.len() != ar * (ac - 1)
            || self.h_vert.len() != ac * (ar - 1)
        {
            anyhow::bail!("bezier warp: anchor/handle counts do not match the cage size");
        }
        for (k, n, name) in [(&self.col_u, ac, "col_u"), (&self.row_v, ar, "row_v")] {
            if !k.is_empty()
                && (k.len() != n || k[0] != 0.0 || k[n - 1] != 1.0 || k.windows(2).any(|w| w[0] >= w[1]))
            {
                anyhow::bail!("bezier warp: {name} must be empty or {n} increasing values from 0 to 1");
            }
        }
        Ok(())
    }

    #[inline]
    fn ac(&self) -> usize {
        self.anchor_cols.max(2) as usize
    }
    #[inline]
    fn ar(&self) -> usize {
        self.anchor_rows.max(2) as usize
    }

    /// Anchor position at grid `(row, col)`.
    pub fn anchor(&self, r: usize, c: usize) -> [f32; 2] {
        self.anchors[r * self.ac() + c]
    }

    /// Source-space `u` of anchor column `c`.
    pub fn col_u(&self, c: usize) -> f32 {
        knot(&self.col_u, c, self.ac())
    }

    /// Source-space `v` of anchor row `r`.
    pub fn row_v(&self, r: usize) -> f32 {
        knot(&self.row_v, r, self.ar())
    }

    fn handle_mut(&mut self, horizontal: bool, r: usize, c: usize) -> &mut Edge {
        let ac = self.ac();
        if horizontal {
            &mut self.h_horiz[r * (ac - 1) + c]
        } else {
            &mut self.h_vert[r * ac + c]
        }
    }

    /// The four outer corners, TL, TR, BR, BL.
    pub fn outer_corners(&self) -> [[f32; 2]; 4] {
        let (ac, ar) = (self.ac(), self.ar());
        [
            self.anchor(0, 0),
            self.anchor(0, ac - 1),
            self.anchor(ar - 1, ac - 1),
            self.anchor(ar - 1, 0),
        ]
    }

    /// Set the outer corners exactly (after a round trip through the
    /// perspective frame) so float drift can't accumulate.
    fn pin_corners(&mut self, outer: [[f32; 2]; 4]) {
        let (ac, ar) = (self.ac(), self.ar());
        for (k, (r, c)) in [(0, 0), (0, ac - 1), (ar - 1, ac - 1), (ar - 1, 0)].into_iter().enumerate() {
            self.anchors[r * ac + c] = outer[k];
        }
    }

    /// Unit square ↔ outer-corner homographies; `None` when bilinear.
    fn frame(&self) -> Option<Frame> {
        if !self.perspective {
            return None;
        }
        let outer = self.outer_corners();
        Some((
            solve_homography(&UNIT_SQUARE, &outer),
            solve_homography(&outer, &UNIT_SQUARE),
        ))
    }

    /// This cage with every anchor and handle mapped through `h`.
    fn mapped(&self, h: &[f32; 9]) -> Self {
        let m = |p: [f32; 2]| apply_homography(h, p);
        Self {
            anchors: self.anchors.iter().map(|&p| m(p)).collect(),
            h_horiz: self.h_horiz.iter().map(|&[a, b]| [m(a), m(b)]).collect(),
            h_vert: self.h_vert.iter().map(|&[a, b]| [m(a), m(b)]).collect(),
            ..self.clone()
        }
    }

    /// Rows and columns swapped. Coons patches are symmetric in `s`/`t`, so
    /// the row operations are the column operations on the transpose.
    fn transposed(&self) -> Self {
        let (ac, ar) = (self.ac(), self.ar());
        Self {
            anchor_cols: ar as u32,
            anchor_rows: ac as u32,
            anchors: (0..ac).flat_map(|c| (0..ar).map(move |r| self.anchor(r, c))).collect(),
            h_horiz: (0..ac).flat_map(|c| (0..ar - 1).map(move |r| self.h_vert[r * ac + c])).collect(),
            h_vert: (0..ac - 1).flat_map(|c| (0..ar).map(move |r| self.h_horiz[r * (ac - 1) + c])).collect(),
            col_u: self.row_v.clone(),
            row_v: self.col_u.clone(),
            ..self.clone()
        }
    }

    fn via_transpose(&mut self, f: impl FnOnce(&mut Self) -> bool) -> bool {
        let mut t = self.transposed();
        let ok = f(&mut t);
        if ok {
            *self = t.transposed();
        }
        ok
    }

    /// Evaluate patch `(pr, pc)` at local `(s, t)` ∈ [0,1]² via Coons
    /// interpolation of its four cubic-bezier boundary edges.
    fn patch_point(&self, pr: usize, pc: usize, s: f32, t: f32) -> [f32; 2] {
        let ac = self.ac();
        let c00 = self.anchor(pr, pc);
        let c10 = self.anchor(pr, pc + 1);
        let c01 = self.anchor(pr + 1, pc);
        let c11 = self.anchor(pr + 1, pc + 1);
        let hh_top = self.h_horiz[pr * (ac - 1) + pc];
        let hh_bot = self.h_horiz[(pr + 1) * (ac - 1) + pc];
        let hv_left = self.h_vert[pr * ac + pc];
        let hv_right = self.h_vert[pr * ac + pc + 1];
        let tp = cubic_point(c00, hh_top[0], hh_top[1], c10, s);
        let bt = cubic_point(c01, hh_bot[0], hh_bot[1], c11, s);
        let l = cubic_point(c00, hv_left[0], hv_left[1], c01, t);
        let r = cubic_point(c10, hv_right[0], hv_right[1], c11, t);
        coons_blend(l, r, tp, bt, c00, c10, c01, c11, s, t)
    }

    /// The warped surface as a function of global `(u, v)` ∈ [0,1]² — what
    /// editors sample to draw the cage's (possibly perspective) curves.
    pub fn surface(&self) -> impl Fn(f32, f32) -> [f32; 2] + use<> {
        let frame = self.frame();
        let cage = match &frame {
            Some((_, inv)) => self.mapped(inv),
            None => self.clone(),
        };
        move |u, v| {
            let (pc, s) = locate(&cage.col_u, cage.ac(), u);
            let (pr, t) = locate(&cage.row_v, cage.ar(), v);
            let p = cage.patch_point(pr, pc, s, t);
            match &frame {
                Some((h, _)) => apply_homography(h, p),
                None => p,
            }
        }
    }

    /// Tessellate the control cage into a dense `WarpMesh`:
    /// `((anchor_cols−1)·tess + 1)` × `((anchor_rows−1)·tess + 1)`, clamped to
    /// `[2, MAX_WARP_SUBDIVISIONS]`. UVs are the uniform unit-square grid. An
    /// invalid cage (see [`Self::validate`]) yields an identity mesh.
    pub fn tessellate(&self) -> WarpMesh {
        if let Err(e) = self.validate() {
            log::warn!("{e}; drawing identity");
            return WarpMesh::identity(2, 2);
        }
        let (ac, ar) = (self.ac() as u32, self.ar() as u32);
        let tess = self.tess.max(1);
        let cols = ((ac - 1) * tess + 1).clamp(2, MAX_WARP_SUBDIVISIONS);
        let rows = ((ar - 1) * tess + 1).clamp(2, MAX_WARP_SUBDIVISIONS);
        let f = self.surface();
        let mut points = Vec::with_capacity((cols * rows) as usize);
        for r in 0..rows {
            let v = r as f32 / (rows - 1) as f32;
            for c in 0..cols {
                let u = c as f32 / (cols - 1) as f32;
                points.push(MeshPoint {
                    position: f(u, v),
                    uv: [u, v],
                });
            }
        }
        WarpMesh { cols, rows, points }
    }

    /// Whether this warp tessellates to an identity mesh (no warp effect).
    pub fn is_identity(&self) -> bool {
        self.tessellate().is_identity()
    }

    /// Move anchor `(r, c)` and the near ends of its handles by `d`.
    fn shift_anchor(&mut self, r: usize, c: usize, d: [f32; 2]) {
        let (ac, ar) = (self.ac(), self.ar());
        let i = r * ac + c;
        self.anchors[i] = add2(self.anchors[i], d);
        let shift = |h: &mut [f32; 2]| *h = add2(*h, d);
        if c + 1 < ac {
            shift(&mut self.h_horiz[r * (ac - 1) + c][0]);
        }
        if c > 0 {
            shift(&mut self.h_horiz[r * (ac - 1) + (c - 1)][1]);
        }
        if r + 1 < ar {
            shift(&mut self.h_vert[r * ac + c][0]);
        }
        if r > 0 {
            shift(&mut self.h_vert[(r - 1) * ac + c][1]);
        }
    }

    /// Move anchor `(row, col)` to `pos`, shifting its tangent handles by the
    /// same delta to keep local curvature. With `perspective`, moving an outer
    /// corner instead re-projects the whole cage — a corner-pin drag that keeps
    /// every curve. Out-of-range indices are ignored.
    pub fn move_anchor(&mut self, r: usize, c: usize, pos: [f32; 2]) {
        let (ac, ar) = (self.ac(), self.ar());
        if r >= ar || c >= ac {
            return;
        }
        let corner = (r == 0 || r == ar - 1) && (c == 0 || c == ac - 1);
        if corner && let Some((_, inv)) = self.frame() {
            let unit = self.mapped(&inv);
            let mut outer = self.outer_corners();
            outer[match (r == 0, c == 0) {
                (true, true) => 0,
                (true, false) => 1,
                (false, false) => 2,
                (false, true) => 3,
            }] = pos;
            *self = unit.mapped(&solve_homography(&UNIT_SQUARE, &outer));
            self.pin_corners(outer);
            return;
        }
        self.shift_anchor(r, c, sub2(pos, self.anchor(r, c)));
    }

    /// Move a group of anchors by `d`. A single anchor behaves exactly like
    /// [`Self::move_anchor`] (a perspective corner re-projects the cage);
    /// several are translated with their handles and the rest of the cage
    /// stays put. Out-of-range entries are ignored.
    pub fn move_anchors(&mut self, sel: &[(usize, usize)], d: [f32; 2]) {
        let (ac, ar) = (self.ac(), self.ar());
        let sel: Vec<_> = sel.iter().copied().filter(|&(r, c)| r < ar && c < ac).collect();
        if let [(r, c)] = sel[..] {
            self.move_anchor(r, c, add2(self.anchor(r, c), d));
            return;
        }
        for (r, c) in sel {
            self.shift_anchor(r, c, d);
        }
    }

    /// Move a single tangent handle to `pos`. `horizontal` selects a horizontal
    /// edge (anchor `(r,c)→(r,c+1)`) vs a vertical edge (`(r,c)→(r+1,c)`);
    /// `which` is 0 (near start anchor) or 1 (near end anchor). With `mirror`,
    /// the handle on the other side of the same anchor turns to stay collinear
    /// (keeping its length), so the curve stays smooth through the anchor.
    /// No-op if the addressed edge does not exist.
    pub fn move_handle(&mut self, horizontal: bool, r: usize, c: usize, which: usize, pos: [f32; 2], mirror: bool) {
        let (ac, ar) = (self.ac(), self.ar());
        let which = which.min(1);
        let exists = if horizontal { r < ar && c + 1 < ac } else { r + 1 < ar && c < ac };
        if !exists {
            return;
        }
        self.handle_mut(horizontal, r, c)[which] = pos;
        if !mirror {
            return;
        }
        // The partner continues this edge's line on the far side of its anchor.
        let (anchor, partner) = match (horizontal, which) {
            (true, 0) => ((r, c), c.checked_sub(1).map(|pc| (r, pc))),
            (true, _) => ((r, c + 1), (c + 2 < ac).then_some((r, c + 1))),
            (false, 0) => ((r, c), r.checked_sub(1).map(|pr| (pr, c))),
            (false, _) => ((r + 1, c), (r + 2 < ar).then_some((r + 1, c))),
        };
        let Some((pr, pc)) = partner else { return };
        let a = self.anchor(anchor.0, anchor.1);
        let dir = sub2(pos, a);
        let len = dir[0].hypot(dir[1]);
        if len < 1e-9 {
            return;
        }
        let other = &mut self.handle_mut(horizontal, pr, pc)[1 - which];
        let keep = sub2(*other, a);
        let keep = keep[0].hypot(keep[1]);
        let keep = if keep < 1e-9 { len } else { keep };
        *other = sub2(a, scale2(dir, keep / len));
    }

    /// Catmull-Rom handles for every edge (central-difference tangents,
    /// one-sided at the cage border), in the perspective frame when there is
    /// one — so an evenly spaced straight grid gets exact straight handles.
    /// ponytail: tangents ignore uneven `col_u`/`row_v` spacing (G1, not C1)
    /// — scale by knot gaps if seams show after many inserts.
    fn smooth_baseline(&self) -> (Vec<Edge>, Vec<Edge>) {
        let (ac, ar) = (self.ac(), self.ar());
        let pts: Vec<[f32; 2]> = match self.frame() {
            Some((_, inv)) => self.anchors.iter().map(|&p| apply_homography(&inv, p)).collect(),
            None => self.anchors.clone(),
        };
        let at = |r: usize, c: usize| pts[r * ac + c];
        let tan = |f: &dyn Fn(usize) -> [f32; 2], i: usize, n: usize| {
            let d = sub2(f((i + 1).min(n - 1)), f(i.saturating_sub(1)));
            scale2(d, if i == 0 || i == n - 1 { 1.0 } else { 0.5 })
        };
        let edge = |p0, t0, p1, t1| [add2(p0, scale2(t0, 1.0 / 3.0)), sub2(p1, scale2(t1, 1.0 / 3.0))];
        let mut hh = Vec::with_capacity(ar * (ac - 1));
        for r in 0..ar {
            let f = |c: usize| at(r, c);
            for c in 0..ac - 1 {
                hh.push(edge(at(r, c), tan(&f, c, ac), at(r, c + 1), tan(&f, c + 1, ac)));
            }
        }
        let mut hv = Vec::with_capacity((ar - 1) * ac);
        for r in 0..ar - 1 {
            for c in 0..ac {
                let f = |r: usize| at(r, c);
                hv.push(edge(at(r, c), tan(&f, r, ar), at(r + 1, c), tan(&f, r + 1, ar)));
            }
        }
        (hh, hv)
    }

    /// Reset every handle to the smooth Catmull-Rom baseline, discarding manual
    /// handle shaping.
    pub fn smooth_all(&mut self) {
        let frame = self.frame();
        let out = |p| frame.as_ref().map_or(p, |(h, _)| apply_homography(h, p));
        let (hh, hv) = self.smooth_baseline();
        self.h_horiz = hh.into_iter().map(|[a, b]| [out(a), out(b)]).collect();
        self.h_vert = hv.into_iter().map(|[a, b]| [out(a), out(b)]).collect();
    }

    /// Run `edit` (anchor moves), then shift every handle by however much its
    /// smooth baseline moved: curves follow the anchors smoothly and any manual
    /// handle shaping is kept as an offset. Baselines are compared in the
    /// perspective frame, so a corner-pin drag (which keeps that frame's shape)
    /// leaves the curves exactly as they were. Skipped if `edit` resizes the cage.
    pub fn with_auto_smooth(&mut self, edit: impl FnOnce(&mut Self)) {
        let dims = (self.ac(), self.ar());
        // Each handle's offset from its baseline, in the (pre-edit) frame.
        let f0 = self.frame();
        let unit = |p| f0.as_ref().map_or(p, |(_, inv)| apply_homography(inv, p));
        let (bh, bv) = self.smooth_baseline();
        let offsets = |hs: &[Edge], base: &[Edge]| -> Vec<Edge> {
            hs.iter().zip(base).map(|(h, b)| [sub2(unit(h[0]), b[0]), sub2(unit(h[1]), b[1])]).collect()
        };
        let (oh, ov) = (offsets(&self.h_horiz, &bh), offsets(&self.h_vert, &bv));
        edit(self);
        if (self.ac(), self.ar()) != dims {
            return;
        }
        // Re-seat every handle on the new baseline with its old offset.
        let f1 = self.frame();
        let out = |p| f1.as_ref().map_or(p, |(h, _)| apply_homography(h, p));
        let (ah, av) = self.smooth_baseline();
        let place = |base: Vec<Edge>, off: Vec<Edge>| -> Vec<Edge> {
            base.into_iter().zip(off).map(|(b, o)| [out(add2(b[0], o[0])), out(add2(b[1], o[1]))]).collect()
        };
        self.h_horiz = place(ah, oh);
        self.h_vert = place(av, ov);
    }

    /// Insert an anchor column at source-space `u` without changing the
    /// surface or the video mapping — the patch split is exact. Returns false
    /// when `u` is on (or within 0.1 % of) an existing column or the cage is
    /// at `MAX_WARP_SUBDIVISIONS`.
    pub fn insert_col(&mut self, u: f32) -> bool {
        let (ac, ar) = (self.ac(), self.ar());
        let (pc, s) = locate(&self.col_u, ac, u);
        if ac >= MAX_WARP_SUBDIVISIONS as usize || !(1e-3..=1.0 - 1e-3).contains(&s) {
            return false;
        }
        let frame = self.frame();
        let outer = self.outer_corners();
        let w = match &frame {
            Some((_, inv)) => self.mapped(inv),
            None => self.clone(),
        };
        let mut anchors = Vec::with_capacity((ac + 1) * ar);
        let mut h_horiz = Vec::with_capacity(ac * ar);
        let mut mids = Vec::with_capacity(ar);
        for r in 0..ar {
            for c in 0..ac - 1 {
                let e = w.h_horiz[r * (ac - 1) + c];
                if c == pc {
                    let (a, m, b) = split_cubic(w.anchor(r, c), e[0], e[1], w.anchor(r, c + 1), s);
                    h_horiz.extend([a, b]);
                    mids.push(m);
                } else {
                    h_horiz.push(e);
                }
            }
            for c in 0..ac {
                anchors.push(w.anchor(r, c));
                if c == pc {
                    anchors.push(mids[r]);
                }
            }
        }
        let mut h_vert = Vec::with_capacity((ac + 1) * (ar - 1));
        for r in 0..ar - 1 {
            for c in 0..ac {
                h_vert.push(w.h_vert[r * ac + c]);
                if c == pc {
                    // The Coons iso-curve at `s` is an exact cubic in `t`: the
                    // left/right edges blended at `s`, plus a linear term that
                    // carries the top/bottom curves' bulge at `s`.
                    let (l, rt) = (w.h_vert[r * ac + c], w.h_vert[r * ac + c + 1]);
                    let ca = sub2(mids[r], lerp2(w.anchor(r, c), w.anchor(r, c + 1), s));
                    let cb = sub2(mids[r + 1], lerp2(w.anchor(r + 1, c), w.anchor(r + 1, c + 1), s));
                    let q = |k: usize| add2(lerp2(l[k], rt[k], s), lerp2(ca, cb, (k + 1) as f32 / 3.0));
                    h_vert.push([q(0), q(1)]);
                }
            }
        }
        let mut col_u: Vec<f32> = (0..ac).map(|c| w.col_u(c)).collect();
        col_u.insert(pc + 1, u);
        let mut out = Self { anchor_cols: ac as u32 + 1, anchors, h_horiz, h_vert, col_u, ..w };
        if let Some((h, _)) = frame {
            out = out.mapped(&h);
            out.pin_corners(outer);
        }
        *self = out;
        true
    }

    /// Insert an anchor row at source-space `v`; see [`Self::insert_col`].
    pub fn insert_row(&mut self, v: f32) -> bool {
        self.via_transpose(|t| t.insert_col(v))
    }

    /// Remove inner anchor column `c`, merging its two patches. Exactly undoes
    /// an `insert_col`; otherwise the merged edges approximate the two curves.
    /// Returns false for an outer column.
    pub fn remove_col(&mut self, c: usize) -> bool {
        let (ac, ar) = (self.ac(), self.ar());
        if c == 0 || c + 1 >= ac {
            return false;
        }
        let frame = self.frame();
        let outer = self.outer_corners();
        let w = match &frame {
            Some((_, inv)) => self.mapped(inv),
            None => self.clone(),
        };
        let (u0, u1, u2) = (w.col_u(c - 1), w.col_u(c), w.col_u(c + 1));
        let s = (u1 - u0) / (u2 - u0);
        let mut anchors = Vec::with_capacity((ac - 1) * ar);
        let mut h_horiz = Vec::with_capacity((ac - 2) * ar);
        for r in 0..ar {
            anchors.extend((0..ac).filter(|&cc| cc != c).map(|cc| w.anchor(r, cc)));
            for cc in 0..ac - 1 {
                let e = w.h_horiz[r * (ac - 1) + cc];
                if cc == c - 1 {
                    // Invert de Casteljau: the split scaled the outer handles
                    // toward their anchors by `s` and `1 − s`.
                    let f = w.h_horiz[r * (ac - 1) + c];
                    let (p0, p3) = (w.anchor(r, c - 1), w.anchor(r, c + 1));
                    h_horiz.push([
                        add2(p0, scale2(sub2(e[0], p0), 1.0 / s)),
                        add2(p3, scale2(sub2(f[1], p3), 1.0 / (1.0 - s))),
                    ]);
                } else if cc != c {
                    h_horiz.push(e);
                }
            }
        }
        let h_vert: Vec<Edge> = (0..ar - 1)
            .flat_map(|r| (0..ac).filter(move |&cc| cc != c).map(move |cc| (r, cc)))
            .map(|(r, cc)| w.h_vert[r * ac + cc])
            .collect();
        let mut col_u: Vec<f32> = (0..ac).map(|cc| w.col_u(cc)).collect();
        col_u.remove(c);
        let mut out = Self { anchor_cols: ac as u32 - 1, anchors, h_horiz, h_vert, col_u, ..w };
        if let Some((h, _)) = frame {
            out = out.mapped(&h);
            out.pin_corners(outer);
        }
        *self = out;
        true
    }

    /// Remove inner anchor row `r`; see [`Self::remove_col`].
    pub fn remove_row(&mut self, r: usize) -> bool {
        self.via_transpose(|t| t.remove_col(r))
    }

    /// Rebuild the cage at `cols × rows` evenly spaced anchors sampled from
    /// the current surface, with smooth handles. Detail between the new
    /// anchors is approximated. Dims clamp to `[2, MAX_WARP_SUBDIVISIONS]`.
    pub fn set_cage_subdivisions(&mut self, cols: u32, rows: u32) {
        let nc = cols.clamp(2, MAX_WARP_SUBDIVISIONS);
        let nr = rows.clamp(2, MAX_WARP_SUBDIVISIONS);
        let f = self.surface();
        let mut anchors = Vec::with_capacity((nc * nr) as usize);
        for r in 0..nr {
            for c in 0..nc {
                anchors.push(f(c as f32 / (nc - 1) as f32, r as f32 / (nr - 1) as f32));
            }
        }
        *self = Self::from_anchors(nc, nr, anchors, self.tess, self.perspective);
        self.smooth_all();
    }

    /// Set the per-patch tessellation density (≥1).
    pub fn set_tess(&mut self, tess: u32) {
        self.tess = tess.max(1);
    }
}

// ── Homography ───────────────────────────────────────────────────────────

/// Compute a forward homography that maps from `src_corners` to `dst_corners`.
/// Returns the 3×3 matrix as a flat, row-major `[f32; 9]`. The vec4 padding for
/// the GPU uniform is applied at the point the uniform is built (see
/// [`WarpStage::new`] / [`WarpStage::set_homography`]).
pub fn compute_forward_homography(
    src_corners: &[[f32; 2]; 4],
    dst_corners: &[[f32; 2]; 4],
) -> [f32; 9] {
    solve_homography(src_corners, dst_corners)
}

fn solve_homography(src: &[[f32; 2]; 4], dst: &[[f32; 2]; 4]) -> [f32; 9] {
    let mut a = [[0.0_f64; 8]; 8];
    let mut b = [0.0_f64; 8];

    for i in 0..4 {
        let (sx, sy) = (src[i][0] as f64, src[i][1] as f64);
        let (dx, dy) = (dst[i][0] as f64, dst[i][1] as f64);
        let row1 = i * 2;
        let row2 = i * 2 + 1;
        a[row1] = [sx, sy, 1.0, 0.0, 0.0, 0.0, -sx * dx, -sy * dx];
        b[row1] = dx;
        a[row2] = [0.0, 0.0, 0.0, sx, sy, 1.0, -sx * dy, -sy * dy];
        b[row2] = dy;
    }

    let h = gauss_solve_8x8(&mut a, &mut b);
    [
        h[0] as f32,
        h[1] as f32,
        h[2] as f32,
        h[3] as f32,
        h[4] as f32,
        h[5] as f32,
        h[6] as f32,
        h[7] as f32,
        1.0,
    ]
}

#[allow(clippy::needless_range_loop)]
fn gauss_solve_8x8(a: &mut [[f64; 8]; 8], b: &mut [f64; 8]) -> [f64; 8] {
    let n = 8;
    for col in 0..n {
        let mut max_row = col;
        let mut max_val = a[col][col].abs();
        for row in (col + 1)..n {
            if a[row][col].abs() > max_val {
                max_val = a[row][col].abs();
                max_row = row;
            }
        }
        a.swap(col, max_row);
        b.swap(col, max_row);

        let pivot = a[col][col];
        if pivot.abs() < 1e-12 {
            log::warn!("Degenerate homography: pivot near zero");
            return [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
        }

        for row in (col + 1)..n {
            let factor = a[row][col] / pivot;
            for k in col..n {
                a[row][k] -= factor * a[col][k];
            }
            b[row] -= factor * b[col];
        }
    }

    let mut x = [0.0_f64; 8];
    for col in (0..n).rev() {
        x[col] = b[col];
        for k in (col + 1)..n {
            x[col] -= a[col][k] * x[k];
        }
        x[col] /= a[col][col];
    }
    x
}

// ── Mesh import/export ───────────────────────────────────────────────────

/// Supported mesh file formats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeshFormat {
    /// Paul Bourke XYUV CSV.
    XyuvCsv,
    /// JSON serialization of WarpMesh.
    Json,
}

impl MeshFormat {
    /// Auto-detect format from file extension.
    pub fn from_extension(path: &std::path::Path) -> Option<Self> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("csv" | "xyuv" | "txt") => Some(Self::XyuvCsv),
            Some("json") => Some(Self::Json),
            _ => None,
        }
    }
}

impl WarpMesh {
    /// Parse a Paul Bourke XYUV CSV string.
    pub fn from_xyuv_csv(input: &str) -> anyhow::Result<Self> {
        let mut lines = input
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#'));

        let header = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("XYUV CSV: missing header"))?;
        let dims: Vec<u32> = header
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        if dims.len() < 2 {
            anyhow::bail!("XYUV CSV: header must contain mesh_w mesh_h");
        }
        let cols = dims[0];
        let rows = dims[1];
        if cols < 2 || rows < 2 {
            anyhow::bail!("XYUV CSV: dimensions must be ≥ 2");
        }
        if cols > 10_000 || rows > 10_000 {
            anyhow::bail!("XYUV CSV: dimensions too large");
        }

        let expected = (cols * rows) as usize;
        let mut points = Vec::with_capacity(expected);

        for line in lines {
            let vals: Vec<f32> = line
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse().ok())
                .collect();
            if vals.len() < 4 {
                continue;
            }
            points.push(MeshPoint {
                position: [vals[0], vals[1]],
                uv: [vals[2], vals[3]],
            });
        }

        if points.len() != expected {
            anyhow::bail!(
                "XYUV CSV: expected {} points ({}×{}), got {}",
                expected,
                cols,
                rows,
                points.len()
            );
        }

        Ok(Self { cols, rows, points })
    }

    /// Export to Paul Bourke XYUV CSV format.
    pub fn to_xyuv_csv(&self) -> String {
        let mut out = String::with_capacity(self.points.len() * 40);
        out.push_str(&format!("{} {}\n", self.cols, self.rows));
        for pt in &self.points {
            out.push_str(&format!(
                "{:.6} {:.6} {:.6} {:.6} 1.000000\n",
                pt.position[0], pt.position[1], pt.uv[0], pt.uv[1],
            ));
        }
        out
    }

    /// Load from file with auto-detected format.
    pub fn load_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let format = MeshFormat::from_extension(path)
            .ok_or_else(|| anyhow::anyhow!("Unknown mesh file extension: {:?}", path))?;
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read mesh file {:?}: {}", path, e))?;
        match format {
            MeshFormat::XyuvCsv => Self::from_xyuv_csv(&content),
            MeshFormat::Json => {
                let mesh: Self = serde_json::from_str(&content)
                    .map_err(|e| anyhow::anyhow!("JSON mesh parse error: {}", e))?;
                if mesh.cols < 2 || mesh.rows < 2 {
                    anyhow::bail!("JSON mesh: dimensions must be ≥ 2");
                }
                if mesh.cols > 10_000 || mesh.rows > 10_000 {
                    anyhow::bail!("JSON mesh: dimensions too large");
                }
                if mesh.points.len() != (mesh.cols * mesh.rows) as usize {
                    anyhow::bail!("JSON mesh: point count mismatch");
                }
                Ok(mesh)
            }
        }
    }

    /// Save to file with auto-detected format.
    pub fn save_file(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let format = MeshFormat::from_extension(path)
            .ok_or_else(|| anyhow::anyhow!("Unknown mesh file extension: {:?}", path))?;
        let content = match format {
            MeshFormat::XyuvCsv => self.to_xyuv_csv(),
            MeshFormat::Json => serde_json::to_string_pretty(self)
                .map_err(|e| anyhow::anyhow!("JSON mesh serialize error: {}", e))?,
        };
        std::fs::write(path, content)
            .map_err(|e| anyhow::anyhow!("Failed to write mesh file {:?}: {}", path, e))?;
        Ok(())
    }
}

// ── WarpStage ────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct WarpParamsUniform {
    h_row0: [f32; 4],
    h_row1: [f32; 4],
    h_row2: [f32; 4],
    use_homography: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
    /// UV crop rectangle: [min_u, min_v, max_u, max_v].
    uv_crop: [f32; 4],
}

/// GPU vertex for warp mesh.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct WarpVertex {
    position: [f32; 2],
    uv: [f32; 2],
}

impl WarpVertex {
    const fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
            ],
        }
    }
}

/// Warp projection stage — mesh or corner-pin UV warping.
pub struct WarpStage {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params_buffer: wgpu::Buffer,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    num_indices: u32,
    cached_bind_group: Option<wgpu::BindGroup>,
    cached_input_ptr: Option<usize>,
    /// Identity warp bypass — blit directly instead of running the warp pass.
    is_identity: bool,
    blit: Option<crate::identity::BlitPipeline>,
    blit_vb: Option<wgpu::Buffer>,
    /// Current UV crop rect [min_u, min_v, max_u, max_v].
    uv_crop: [f32; 4],
    /// Last homography (for cheap corner-pin updates).
    last_homography: [f32; 9],
    /// Last use_homography value (1.0 = corner-pin, 0.0 = mesh).
    last_use_homography: f32,
}

impl WarpStage {
    /// Create a warp stage from a `WarpMesh`.
    pub fn from_mesh(device: &wgpu::Device, format: wgpu::TextureFormat, mesh: &WarpMesh) -> Self {
        let (vertices, indices) = build_mesh_buffers(mesh);
        let mut s = Self::new(device, format, &vertices, &indices, false, None);
        if mesh.is_identity() {
            s.set_identity_blit(device, format);
        }
        s
    }

    /// Create a warp stage from a `WarpMode`.
    pub fn from_mode(device: &wgpu::Device, format: wgpu::TextureFormat, mode: &WarpMode) -> Self {
        let mut s = match mode {
            WarpMode::CornerPin { corners } => {
                // Corner-pin uses a unit-square vertex buffer; the homography
                // maps src [0,1]² → dst corners in the vertex shader.
                let unit_square = WarpMesh::identity(2, 2);
                let (vertices, indices) = build_mesh_buffers(&unit_square);
                let src = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
                let h = compute_forward_homography(&src, corners);
                Self::new(device, format, &vertices, &indices, true, Some(&h))
            }
            WarpMode::Mesh(mesh) => {
                let (vertices, indices) = build_mesh_buffers(mesh);
                Self::new(device, format, &vertices, &indices, false, None)
            }
            WarpMode::Bezier(b) => {
                let (vertices, indices) = build_mesh_buffers(&b.tessellate());
                Self::new(device, format, &vertices, &indices, false, None)
            }
        };
        if mode.is_identity() {
            s.set_identity_blit(device, format);
        }
        s
    }

    fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        vertices: &[WarpVertex],
        indices: &[u16],
        is_corner_pin: bool,
        initial_homography: Option<&[f32; 9]>,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Warp Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/warp.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Warp BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Warp Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Warp Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(WarpVertex::desc())],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Warp Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let h = initial_homography
            .copied()
            .unwrap_or([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Warp Params"),
            contents: bytemuck::bytes_of(&WarpParamsUniform {
                h_row0: [h[0], h[1], h[2], 0.0],
                h_row1: [h[3], h[4], h[5], 0.0],
                h_row2: [h[6], h[7], h[8], 0.0],
                use_homography: if is_corner_pin { 1.0 } else { 0.0 },
                _pad0: 0.0,
                _pad1: 0.0,
                _pad2: 0.0,
                uv_crop: [0.0, 0.0, 1.0, 1.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Warp Vertex Buffer"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Warp Index Buffer"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            params_buffer,
            vertex_buffer,
            index_buffer,
            num_indices: indices.len() as u32,
            cached_bind_group: None,
            cached_input_ptr: None,
            is_identity: false,
            blit: None,
            blit_vb: None,
            uv_crop: [0.0, 0.0, 1.0, 1.0],
            last_homography: h,
            last_use_homography: if is_corner_pin { 1.0 } else { 0.0 },
        }
    }

    fn set_identity_blit(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        self.is_identity = true;
        self.blit = Some(crate::identity::BlitPipeline::new(device, format));
        use wgpu::util::DeviceExt;
        let vertices: &[crate::identity::BlitVertex] = &[
            crate::identity::BlitVertex {
                position: [-1.0, -1.0],
                texcoord: [0.0, 1.0],
            },
            crate::identity::BlitVertex {
                position: [1.0, -1.0],
                texcoord: [1.0, 1.0],
            },
            crate::identity::BlitVertex {
                position: [-1.0, 1.0],
                texcoord: [0.0, 0.0],
            },
            crate::identity::BlitVertex {
                position: [-1.0, 1.0],
                texcoord: [0.0, 0.0],
            },
            crate::identity::BlitVertex {
                position: [1.0, -1.0],
                texcoord: [1.0, 1.0],
            },
            crate::identity::BlitVertex {
                position: [1.0, 1.0],
                texcoord: [1.0, 0.0],
            },
        ];
        self.blit_vb = Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Warp Identity Blit VB"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            }),
        );
    }

    /// Update the homography for corner-pin mode.
    pub fn set_homography(&mut self, queue: &wgpu::Queue, homography: &[f32; 9]) {
        self.is_identity = false;
        self.blit = None;
        self.blit_vb = None;
        self.last_homography = *homography;
        self.last_use_homography = 1.0;
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::bytes_of(&WarpParamsUniform {
                h_row0: [homography[0], homography[1], homography[2], 0.0],
                h_row1: [homography[3], homography[4], homography[5], 0.0],
                h_row2: [homography[6], homography[7], homography[8], 0.0],
                use_homography: 1.0,
                _pad0: 0.0,
                _pad1: 0.0,
                _pad2: 0.0,
                uv_crop: self.uv_crop,
            }),
        );
    }

    /// Update the UV crop rectangle. `crop` is `[min_u, min_v, max_u, max_v]`.
    pub fn set_uv_crop(&mut self, queue: &wgpu::Queue, crop: &[f32; 4]) {
        self.uv_crop = *crop;
        // Disable identity blit when cropping — the blit pipeline doesn't
        // support UV cropping, so we must run the warp shader.
        if self.uv_crop != [0.0, 0.0, 1.0, 1.0] {
            self.is_identity = false;
            self.blit = None;
            self.blit_vb = None;
        }
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::bytes_of(&WarpParamsUniform {
                h_row0: [self.last_homography[0], self.last_homography[1], self.last_homography[2], 0.0],
                h_row1: [self.last_homography[3], self.last_homography[4], self.last_homography[5], 0.0],
                h_row2: [self.last_homography[6], self.last_homography[7], self.last_homography[8], 0.0],
                use_homography: self.last_use_homography,
                _pad0: 0.0,
                _pad1: 0.0,
                _pad2: 0.0,
                uv_crop: self.uv_crop,
            }),
        );
    }

    /// Cheap in-place mesh update — updates vertex positions without rebuilding
    /// the pipeline. The mesh dimensions (cols × rows) must match the existing
    /// vertex buffer size (true for drag editing).
    pub fn set_mesh(&mut self, queue: &wgpu::Queue, mesh: &WarpMesh) {
        self.is_identity = false;
        self.blit = None;
        self.blit_vb = None;
        self.last_use_homography = 0.0;

        let vertices: Vec<WarpVertex> = mesh
            .points
            .iter()
            .map(|p| WarpVertex {
                position: p.position,
                uv: p.uv,
            })
            .collect();

        queue.write_buffer(
            &self.vertex_buffer,
            0,
            bytemuck::cast_slice(&vertices),
        );
    }
}

impl ProjectionStage for WarpStage {
    fn label(&self) -> &str {
        "warp"
    }

    fn render(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        input: &wgpu::TextureView,
        _input_texture: Option<&wgpu::Texture>,
        output: &wgpu::TextureView,
        _output_size: [u32; 2],
    ) {
        if self.is_identity
            && let (Some(blit), Some(vb)) = (self.blit.as_ref(), self.blit_vb.as_ref()) {
                let bg = blit.create_bind_group(ctx.device, input);
                blit.blit(ctx.encoder, &bg, output, vb);
                return;
            }
        self.draw(ctx, input, output, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
    }

    fn on_input_changed(&mut self, _device: &wgpu::Device, _size: [u32; 2]) {
        self.cached_bind_group = None;
        self.cached_input_ptr = None;
    }
}

impl WarpStage {
    /// Draw over whatever `output` already holds instead of clearing it, so
    /// several warps layer into one frame — a projector showing more than one
    /// surface. Always runs the warp pass: the identity blit clears.
    pub fn render_over(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
    ) {
        self.draw(ctx, input, output, wgpu::LoadOp::Load);
    }

    fn draw(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
        load: wgpu::LoadOp<wgpu::Color>,
    ) {
        let input_ptr = input as *const _ as usize;
        let bind_group = if self.cached_input_ptr == Some(input_ptr) {
            self.cached_bind_group.as_ref().unwrap()
        } else {
            let bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Warp Bind Group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.params_buffer.as_entire_binding(),
                    },
                ],
            });
            self.cached_bind_group = Some(bg);
            self.cached_input_ptr = Some(input_ptr);
            self.cached_bind_group.as_ref().unwrap()
        };

        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Warp Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw_indexed(0..self.num_indices, 0, 0..1);
    }
}

fn build_mesh_buffers(mesh: &WarpMesh) -> (Vec<WarpVertex>, Vec<u16>) {
    let vertices: Vec<WarpVertex> = mesh
        .points
        .iter()
        .map(|p| WarpVertex {
            position: p.position,
            uv: p.uv,
        })
        .collect();

    let mut indices = Vec::new();
    let cols = mesh.cols as usize;
    let rows = mesh.rows as usize;
    for r in 0..rows - 1 {
        for c in 0..cols - 1 {
            let i0 = (r * cols + c) as u16;
            let i1 = i0 + 1;
            let i2 = ((r + 1) * cols + c) as u16;
            let i3 = i2 + 1;
            // Two triangles per cell
            indices.extend_from_slice(&[i0, i1, i2, i1, i3, i2]);
        }
    }

    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_matches_homography(b: &BezierWarp, corners: &[[f32; 2]; 4]) {
        let h = compute_forward_homography(&UNIT_SQUARE, corners);
        for p in &b.tessellate().points {
            let want = apply_homography(&h, p.uv);
            let d = (p.position[0] - want[0]).abs().max((p.position[1] - want[1]).abs());
            assert!(d < 1e-5, "uv {:?}: got {:?}, want {:?}", p.uv, p.position, want);
        }
    }

    #[test]
    fn straight_perspective_bezier_is_exact_corner_pin() {
        let corners = [[0.1, 0.05], [0.95, 0.2], [0.8, 0.9], [0.05, 0.7]];
        let mut b = BezierWarp::corner_pin(corners, 8);
        assert_matches_homography(&b, &corners);

        // Dragging a corner stays an exact corner-pin of the new quad.
        b.move_anchor(1, 1, [0.9, 0.95]);
        assert_matches_homography(&b, &[corners[0], corners[1], [0.9, 0.95], corners[3]]);

        // ...and so does a finer cage resampled from it.
        b.set_cage_subdivisions(4, 3);
        assert!(b.validate().is_ok());
        assert_matches_homography(&b, &[corners[0], corners[1], [0.9, 0.95], corners[3]]);
    }

    fn assert_same_surface(a: &BezierWarp, b: &BezierWarp) {
        let (fa, fb) = (a.surface(), b.surface());
        for i in 0..=20 {
            for j in 0..=20 {
                let (u, v) = (i as f32 / 20.0, j as f32 / 20.0);
                let (p, q) = (fa(u, v), fb(u, v));
                let d = (p[0] - q[0]).abs().max((p[1] - q[1]).abs());
                assert!(d < 1e-4, "({u}, {v}): {p:?} vs {q:?}");
            }
        }
    }

    #[test]
    fn inserting_rows_and_cols_is_exact_and_removing_undoes_it() {
        let anchors = vec![
            [0.0, 0.0], [0.5, 0.05], [1.0, 0.0],
            [0.05, 0.5], [0.5, 0.5], [0.95, 0.55],
            [0.0, 1.0], [0.5, 0.9], [1.0, 1.0],
        ];
        for perspective in [false, true] {
            let mut b = BezierWarp::from_anchors(3, 3, anchors.clone(), 8, perspective);
            b.move_handle(true, 0, 0, 0, [0.2, -0.1], false);
            b.move_handle(false, 1, 2, 1, [1.1, 0.8], false);
            let original = b.clone();

            assert!(b.insert_col(0.3) && b.insert_row(0.8) && b.insert_col(0.62));
            assert!(!b.insert_col(0.62), "an existing column is not re-inserted");
            assert!(b.validate().is_ok());
            assert_eq!((b.anchor_cols, b.anchor_rows), (5, 4));
            assert_same_surface(&original, &b);

            let col = |b: &BezierWarp, u: f32| (0..5).find(|&c| b.col_u(c) == u).unwrap();
            assert!(b.remove_col(col(&b, 0.62)) && b.remove_col(col(&b, 0.3)));
            assert!(b.remove_row((0..4).find(|&r| b.row_v(r) == 0.8).unwrap()));
            assert!(!b.remove_col(0), "outer columns stay");
            assert_same_surface(&original, &b);
        }
    }

    #[test]
    fn auto_smooth_keeps_corner_pins_exact_and_mirror_keeps_handles_collinear() {
        let corners = [[0.1, 0.05], [0.95, 0.2], [0.8, 0.9], [0.05, 0.7]];
        let mut b = BezierWarp::corner_pin(corners, 8);
        b.set_cage_subdivisions(4, 3);
        b.with_auto_smooth(|w| w.move_anchors(&[(2, 3)], [0.05, 0.02]));
        assert_matches_homography(&b, &[corners[0], corners[1], [0.85, 0.92], corners[3]]);

        let cross = |a: [f32; 2], p: [f32; 2], q: [f32; 2]| {
            let (x, y) = (sub2(p, a), sub2(q, a));
            (x[0] * y[1] - x[1] * y[0], x[0] * y[0] + x[1] * y[1])
        };
        // Dragging an inner anchor with auto-smooth leaves the row smooth through it.
        b.with_auto_smooth(|w| w.move_anchors(&[(1, 1)], [0.03, -0.04]));
        let (c, dot) = cross(b.anchor(1, 1), b.h_horiz[3][1], b.h_horiz[4][0]);
        assert!(c.abs() < 1e-6 && dot < 0.0, "kink at (1,1): cross {c}");

        // A mirrored handle drag swings the partner to stay collinear.
        b.move_handle(true, 1, 1, 0, [0.6, 0.3], true);
        let (c, dot) = cross(b.anchor(1, 1), b.h_horiz[4][0], b.h_horiz[3][1]);
        assert!(c.abs() < 1e-6 && dot < 0.0, "mirror broke at (1,1): cross {c}");
    }

    #[test]
    fn bezier_warp_mode_round_trips_and_loads_varda_shape() {
        let mode = WarpMode::Bezier(BezierWarp::corner_pin(UNIT_SQUARE, 4));
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(serde_json::from_str::<WarpMode>(&json).unwrap(), mode);

        // varda files have no `perspective` field → bilinear.
        let varda = json.replace(",\"perspective\":true", "");
        let WarpMode::Bezier(b) = serde_json::from_str::<WarpMode>(&varda).unwrap() else {
            panic!("not bezier")
        };
        assert!(!b.perspective);
        assert!(b.is_identity());
    }

    #[test]
    fn identity_mesh_positions_equal_uvs() {
        let mesh = WarpMesh::identity(4, 4);
        assert_eq!(mesh.points.len(), 16);
        assert!(mesh.is_identity());
    }

    #[test]
    fn from_corners_creates_2x2_mesh() {
        let corners = [[0.1, 0.2], [0.9, 0.2], [0.9, 0.8], [0.1, 0.8]];
        let mesh = WarpMesh::from_corners(&corners);
        assert_eq!(mesh.cols, 2);
        assert_eq!(mesh.rows, 2);
        assert_eq!(mesh.points[0].position, corners[0]);
        assert_eq!(mesh.points[0].uv, [0.0, 0.0]);
    }

    #[test]
    fn xyuv_csv_roundtrip() {
        let mesh = WarpMesh::identity(3, 3);
        let csv = mesh.to_xyuv_csv();
        let parsed = WarpMesh::from_xyuv_csv(&csv).unwrap();
        assert_eq!(parsed.cols, 3);
        assert_eq!(parsed.rows, 3);
        assert!(parsed.is_identity());
    }

    #[test]
    fn identity_homography_is_identity_matrix() {
        let src = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let dst = src;
        let h = compute_forward_homography(&src, &dst);
        // Flat row-major 3×3: diagonal is indices 0, 4, 8.
        assert!((h[0] - 1.0).abs() < 1e-4);
        assert!((h[4] - 1.0).abs() < 1e-4);
        assert!((h[8] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn warp_mode_identity() {
        assert!(WarpMode::identity().is_identity());
        assert!(
            !WarpMode::corner_pin([[0.1, 0.0], [0.9, 0.0], [1.0, 1.0], [0.0, 1.0]]).is_identity()
        );
    }

    #[test]
    fn warp_identity_snapshot() {
        let (device, queue) = pollster::block_on(crate::test_harness::init_wgpu());
        let (_input_tex, input_view) =
            crate::test_harness::create_checkerboard_texture(&device, &queue);
        let (_output_tex, output_view) = crate::test_harness::create_output_texture(&device, 2, 2);

        let mesh = WarpMesh::identity(2, 2);
        let mut stage = WarpStage::from_mesh(&device, wgpu::TextureFormat::Rgba8Unorm, &mesh);
        crate::test_harness::run_stage(
            &device,
            &queue,
            &mut stage,
            &input_view,
            Some(&_input_tex),
            &output_view,
            [2, 2],
        );

        let pixels = crate::test_harness::readback_rgba8(&device, &queue, &_output_tex, 2, 2);
        // Identity warp should reproduce the checkerboard (linear sampling at pixel centers
        // on a matched-resolution texture is exact).
        assert_eq!(&pixels[0..4], &[255, 255, 255, 255]);
        assert_eq!(&pixels[4..8], &[0, 0, 0, 255]);
        assert_eq!(&pixels[8..12], &[0, 0, 0, 255]);
        assert_eq!(&pixels[12..16], &[255, 255, 255, 255]);
    }

    /// Two surfaces on one projector: the second draw must land beside the
    /// first, not clear it.
    #[test]
    fn render_over_layers_without_clearing() {
        let (device, queue) = pollster::block_on(crate::test_harness::init_wgpu());
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let (red_tex, red) =
            crate::test_harness::create_solid_texture(&device, &queue, 4, 4, [255, 0, 0, 255]);
        let (_green_tex, green) =
            crate::test_harness::create_solid_texture(&device, &queue, 4, 4, [0, 255, 0, 255]);
        let (out_tex, out) = crate::test_harness::create_output_texture(&device, 4, 4);

        let left = WarpMode::corner_pin([[0.0, 0.0], [0.5, 0.0], [0.5, 1.0], [0.0, 1.0]]);
        let right = WarpMode::corner_pin([[0.5, 0.0], [1.0, 0.0], [1.0, 1.0], [0.5, 1.0]]);
        let mut first = WarpStage::from_mode(&device, format, &left);
        let mut second = WarpStage::from_mode(&device, format, &right);

        crate::test_harness::run_stage(&device, &queue, &mut first, &red, Some(&red_tex), &out, [4, 4]);
        let mut encoder = device.create_command_encoder(&Default::default());
        let dummy_vb = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 64,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        let mut ctx = RenderCtx {
            device: &device,
            queue: &queue,
            encoder: &mut encoder,
            vertex_buffer: &dummy_vb,
        };
        second.render_over(&mut ctx, &green, &out);
        queue.submit(std::iter::once(encoder.finish()));

        let px = crate::test_harness::readback_rgba8(&device, &queue, &out_tex, 4, 4);
        let row1 = 4 * 4;
        assert_eq!(&px[row1..row1 + 4], &[255, 0, 0, 255], "left survives");
        assert_eq!(&px[row1 + 12..row1 + 16], &[0, 255, 0, 255], "right drawn over");
    }

    #[test]
    fn warp_corner_pin_math() {
        let (device, queue) = pollster::block_on(crate::test_harness::init_wgpu());
        let (_input_tex, input_view) =
            crate::test_harness::create_solid_texture(&device, &queue, 4, 4, [255, 255, 255, 255]);
        let (_output_tex, output_view) = crate::test_harness::create_output_texture(&device, 4, 4);

        // Trapezoid: top edge narrower than bottom edge.
        let corners = [
            [0.25, 0.0], // TL
            [0.75, 0.0], // TR
            [1.0, 1.0],  // BR
            [0.0, 1.0],  // BL
        ];
        let mode = WarpMode::CornerPin { corners };
        let mut stage = WarpStage::from_mode(&device, wgpu::TextureFormat::Rgba8Unorm, &mode);
        crate::test_harness::run_stage(
            &device,
            &queue,
            &mut stage,
            &input_view,
            Some(&_input_tex),
            &output_view,
            [4, 4],
        );

        let pixels = crate::test_harness::readback_rgba8(&device, &queue, &_output_tex, 4, 4);

        // Top-left corner of the 4×4 output should be black (outside the trapezoid).
        assert_eq!(&pixels[0..4], &[0, 0, 0, 255]);

        // The center of the top row (x≈0.5) is inside the narrow top edge.
        let top_center = &pixels[8..12];
        assert!(
            top_center[0] > 200,
            "top-center should be white, got {:?}",
            top_center
        );

        // Bottom row spans the full width, so bottom-left should be white.
        let bottom_left = &pixels[(3 * 4 * 4)..(3 * 4 * 4 + 4)];
        assert!(
            bottom_left[0] > 200,
            "bottom-left should be white, got {:?}",
            bottom_left
        );
    }
}
