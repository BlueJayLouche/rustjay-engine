//! Blob detection on a single-channel (luma) frame.
//!
//! Threshold → 4-connected components (iterative flood fill) →
//! intensity-weighted subpixel centroid. Dependency-free; the caller converts a
//! captured frame (e.g. BGRA from nokhwa) to luma.
//!
//! `// ponytail:` deliberately not OpenCV/imageproc. A flood fill over a
//! thresholded mask is enough for sparse calibration blobs. Upgrade path: swap
//! [`detect_blobs`] for `imageproc::region_labelling` only if dense/overlapping
//! LEDs measurably defeat this.

/// A detected bright region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blob {
    /// Intensity-weighted centroid X, in pixels (subpixel).
    pub x: f32,
    /// Intensity-weighted centroid Y, in pixels (subpixel).
    pub y: f32,
    /// Number of pixels above threshold.
    pub area: u32,
    /// Sum of (luma − threshold) over the blob; a brightness proxy.
    pub weight: u64,
}

/// Find all blobs whose pixels exceed `threshold`.
///
/// `luma` is row-major, length `w * h`. Single-pixel specks (`area < 2`) are
/// dropped as noise.
pub fn detect_blobs(luma: &[u8], w: usize, h: usize, threshold: u8) -> Vec<Blob> {
    assert_eq!(luma.len(), w * h, "luma length must be w*h");
    let mut visited = vec![false; luma.len()];
    let mut blobs = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    for start in 0..luma.len() {
        if visited[start] || luma[start] <= threshold {
            continue;
        }
        // Flood-fill this connected component.
        let mut sum_w: u64 = 0;
        let mut sum_x: f64 = 0.0;
        let mut sum_y: f64 = 0.0;
        let mut area: u32 = 0;
        stack.clear();
        stack.push(start);
        visited[start] = true;

        while let Some(idx) = stack.pop() {
            let val = luma[idx];
            if val <= threshold {
                continue;
            }
            let px = idx % w;
            let py = idx / w;
            let weight = (val - threshold) as u64;
            sum_w += weight;
            sum_x += px as f64 * weight as f64;
            sum_y += py as f64 * weight as f64;
            area += 1;

            // 4-connected neighbours.
            if px > 0 { push_if(&mut stack, &mut visited, luma, threshold, idx - 1); }
            if px + 1 < w { push_if(&mut stack, &mut visited, luma, threshold, idx + 1); }
            if py > 0 { push_if(&mut stack, &mut visited, luma, threshold, idx - w); }
            if py + 1 < h { push_if(&mut stack, &mut visited, luma, threshold, idx + w); }
        }

        if area >= 2 && sum_w > 0 {
            blobs.push(Blob {
                x: (sum_x / sum_w as f64) as f32,
                y: (sum_y / sum_w as f64) as f32,
                area,
                weight: sum_w,
            });
        }
    }
    blobs
}

#[inline]
fn push_if(stack: &mut Vec<usize>, visited: &mut [bool], luma: &[u8], threshold: u8, idx: usize) {
    if !visited[idx] && luma[idx] > threshold {
        visited[idx] = true;
        stack.push(idx);
    }
}

/// The single brightest blob (largest `weight`), if any exceed `threshold`.
///
/// Sequential-flash calibration expects exactly one lit LED per frame, so the
/// brightest blob is that LED.
pub fn brightest_blob(luma: &[u8], w: usize, h: usize, threshold: u8) -> Option<Blob> {
    detect_blobs(luma, w, h, threshold)
        .into_iter()
        .max_by_key(|b| b.weight)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 bright square at a known location yields a centroid at its middle.
    #[test]
    fn centroid_of_square() {
        let (w, h) = (10, 10);
        let mut luma = vec![0u8; w * h];
        for (px, py) in [(4, 5), (5, 5), (4, 6), (5, 6)] {
            luma[py * w + px] = 255;
        }
        let b = brightest_blob(&luma, w, h, 32).expect("one blob");
        assert!((b.x - 4.5).abs() < 1e-4, "x={}", b.x);
        assert!((b.y - 5.5).abs() < 1e-4, "y={}", b.y);
        assert_eq!(b.area, 4);
    }

    /// Two separated squares are two distinct blobs; brightest wins.
    #[test]
    fn two_blobs_pick_brightest() {
        let (w, h) = (20, 10);
        let mut luma = vec![0u8; w * h];
        for (px, py) in [(2, 2), (3, 2)] { luma[py * w + px] = 100; }
        for (px, py) in [(15, 7), (16, 7)] { luma[py * w + px] = 255; }
        assert_eq!(detect_blobs(&luma, w, h, 32).len(), 2);
        let b = brightest_blob(&luma, w, h, 32).unwrap();
        assert!(b.x > 14.0, "brightest should be the right pair, x={}", b.x);
    }

    #[test]
    fn nothing_above_threshold_is_none() {
        let luma = vec![10u8; 16];
        assert!(brightest_blob(&luma, 4, 4, 32).is_none());
    }
}
