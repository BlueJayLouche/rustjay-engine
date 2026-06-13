//! Scan-order demuxing for atlas tiles.
//!
//! Fixtures in a segment are not always stored left-to-right, top-to-bottom.
//! LED strips are often wired zig-zag (serpentine) and may start from any
//! corner. This module maps a tile's pixels into fixture order.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanOrder {
    pub start_corner: Corner,
    pub serpentine: bool,
    pub primary: Axis,
}

impl Default for ScanOrder {
    fn default() -> Self {
        Self {
            start_corner: Corner::TopLeft,
            serpentine: false,
            primary: Axis::Horizontal,
        }
    }
}

impl Corner {
    pub fn label(&self) -> &'static str {
        match self {
            Corner::TopLeft => "TL",
            Corner::TopRight => "TR",
            Corner::BottomLeft => "BL",
            Corner::BottomRight => "BR",
        }
    }
}

impl Axis {
    pub fn label(&self) -> &'static str {
        match self {
            Axis::Horizontal => "Horiz",
            Axis::Vertical => "Vert",
        }
    }
}

/// Demux one atlas tile into fixture-order BGRA pixels.
///
/// `bgra` is the tightly-packed atlas readback (`width × height × 4` bytes).
/// `tile_offset` and `tile_size` are in pixels. Returns an empty vec if the tile
/// lies outside the atlas data.
pub fn demux_tile(
    bgra: &[u8],
    atlas_width: u32,
    tile_offset: [u32; 2],
    tile_size: [u32; 2],
    order: ScanOrder,
) -> Vec<[u8; 4]> {
    let [cols, rows] = [tile_size[0].max(1), tile_size[1].max(1)];
    let count = (cols * rows) as usize;
    let mut out = Vec::with_capacity(count);

    let positions = tile_positions(cols, rows, order);
    for (col, row) in positions {
        let x = tile_offset[0] + col;
        let y = tile_offset[1] + row;
        let idx = ((y * atlas_width + x) * 4) as usize;
        let pixel = if idx + 3 <= bgra.len() {
            [bgra[idx], bgra[idx + 1], bgra[idx + 2], bgra[idx + 3]]
        } else {
            [0, 0, 0, 255]
        };
        out.push(pixel);
    }
    out
}

fn tile_positions(cols: u32, rows: u32, order: ScanOrder) -> Vec<(u32, u32)> {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let mut out = Vec::with_capacity((cols * rows) as usize);
    let start_left = matches!(
        order.start_corner,
        Corner::TopLeft | Corner::BottomLeft
    );
    let start_top = matches!(
        order.start_corner,
        Corner::TopLeft | Corner::TopRight
    );

    match order.primary {
        Axis::Horizontal => {
            for row_idx in 0..rows {
                let row = if start_top {
                    row_idx
                } else {
                    rows - 1 - row_idx
                };
                let reverse = order.serpentine && (row_idx % 2 == 1);
                for col_idx in 0..cols {
                    let col = if start_left ^ reverse {
                        col_idx
                    } else {
                        cols - 1 - col_idx
                    };
                    out.push((col, row));
                }
            }
        }
        Axis::Vertical => {
            for col_idx in 0..cols {
                let col = if start_left {
                    col_idx
                } else {
                    cols - 1 - col_idx
                };
                let reverse = order.serpentine && (col_idx % 2 == 1);
                for row_idx in 0..rows {
                    let row = if start_top ^ reverse {
                        row_idx
                    } else {
                        rows - 1 - row_idx
                    };
                    out.push((col, row));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_atlas(cols: u32, rows: u32) -> (Vec<u8>, Vec<[u8; 4]>) {
        let mut bgra = Vec::with_capacity((cols * rows * 4) as usize);
        let mut expected = Vec::with_capacity((cols * rows) as usize);
        for y in 0..rows {
            for x in 0..cols {
                let b = (x % 256) as u8;
                let g = (y % 256) as u8;
                let r = ((x + y) % 256) as u8;
                let a = 255u8;
                bgra.extend_from_slice(&[b, g, r, a]);
                expected.push([b, g, r, a]);
            }
        }
        (bgra, expected)
    }

    #[test]
    fn top_left_horizontal_no_serpentine() {
        let (bgra, expected) = make_atlas(3, 2);
        let order = ScanOrder::default();
        let out = demux_tile(&bgra, 3, [0, 0], [3, 2], order);
        assert_eq!(out, expected);
    }

    #[test]
    fn top_left_horizontal_serpentine() {
        let (bgra, _) = make_atlas(3, 2);
        let order = ScanOrder {
            start_corner: Corner::TopLeft,
            serpentine: true,
            primary: Axis::Horizontal,
        };
        let out = demux_tile(&bgra, 3, [0, 0], [3, 2], order);
        // Row 0: 0,1,2; Row 1: 5,4,3
        assert_eq!(out[0], [0, 0, 0, 255]);
        assert_eq!(out[2], [2, 0, 2, 255]);
        assert_eq!(out[3], [2, 1, 3, 255]);
        assert_eq!(out[5], [0, 1, 1, 255]);
    }

    #[test]
    fn bottom_right_vertical_serpentine() {
        let (bgra, _) = make_atlas(2, 3);
        let order = ScanOrder {
            start_corner: Corner::BottomRight,
            serpentine: true,
            primary: Axis::Vertical,
        };
        let out = demux_tile(&bgra, 2, [0, 0], [2, 3], order);
        // Col 1 bottom->top: (1,2),(1,1),(1,0); Col 0 top->bottom: (0,0),(0,1),(0,2)
        assert_eq!(out[0], [1, 2, 3, 255]);
        assert_eq!(out[2], [1, 0, 1, 255]);
        assert_eq!(out[3], [0, 0, 0, 255]);
        assert_eq!(out[5], [0, 2, 2, 255]);
    }
}
