//! DMX universe/channel overlap detection for lighting patches.

use crate::dmx::DMX_UNIVERSE_SIZE;

/// A contiguous channel range inside one universe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchSpan {
    /// Human-readable owner, e.g. output name.
    pub owner: String,
    /// Human-readable segment/profile name for detail.
    pub detail: String,
    /// 1-based DMX universe.
    pub universe: u16,
    /// 1-based first channel (inclusive).
    pub start: u16,
    /// 1-based last channel (inclusive).
    pub end: u16,
}

impl PatchSpan {
    /// Whether this span overlaps another in the same universe.
    pub fn overlaps(&self, other: &PatchSpan) -> bool {
        self.universe == other.universe && self.start <= other.end && other.start <= self.end
    }
}

/// A detected overlap between two spans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlap {
    pub a: PatchSpan,
    pub b: PatchSpan,
    pub universe: u16,
    pub start: u16,
    pub end: u16,
}

/// Compute the universe/channel spans occupied by a sequential segment.
///
/// Fixtures are laid out starting at `start_universe`/`start_channel` (1-based)
/// with the given `footprint`. When a fixture would overflow universe 512,
/// the patch advances to the next universe and channel resets to 1.
pub fn segment_spans(
    owner: impl Into<String>,
    detail: impl Into<String>,
    start_universe: u16,
    start_channel: u16,
    footprint: usize,
    fixture_count: usize,
) -> Vec<PatchSpan> {
    let owner = owner.into();
    let detail = detail.into();
    if footprint == 0 || fixture_count == 0 {
        return Vec::new();
    }
    let footprint = footprint.min(DMX_UNIVERSE_SIZE);
    let mut spans: Vec<PatchSpan> = Vec::new();
    let mut universe = start_universe.max(1);
    let mut ch = start_channel.max(1) as usize;

    for _ in 0..fixture_count {
        if ch + footprint - 1 > DMX_UNIVERSE_SIZE {
            universe = universe.wrapping_add(1);
            ch = 1;
        }
        let end = (ch + footprint - 1).min(DMX_UNIVERSE_SIZE) as u16;
        if let Some(last) = spans.last_mut() {
            if last.universe == universe && last.end + 1 == ch as u16 {
                last.end = end;
            } else {
                spans.push(PatchSpan {
                    owner: owner.clone(),
                    detail: detail.clone(),
                    universe,
                    start: ch as u16,
                    end,
                });
            }
        } else {
            spans.push(PatchSpan {
                owner: owner.clone(),
                detail: detail.clone(),
                universe,
                start: ch as u16,
                end,
            });
        }
        ch += footprint;
    }
    spans
}

/// Find every pair of overlapping spans. The result is deterministic:
/// spans are compared in input order, and for each pair only one `Overlap`
/// is emitted (i < j).
pub fn find_overlaps(spans: &[PatchSpan]) -> Vec<Overlap> {
    let mut out = Vec::new();
    for i in 0..spans.len() {
        for j in (i + 1)..spans.len() {
            let a = &spans[i];
            let b = &spans[j];
            if a.overlaps(b) {
                let start = a.start.max(b.start);
                let end = a.end.min(b.end);
                out.push(Overlap {
                    a: a.clone(),
                    b: b.clone(),
                    universe: a.universe,
                    start,
                    end,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_overlap_disjoint_universes() {
        let spans = vec![
            PatchSpan {
                owner: "a".into(),
                detail: "".into(),
                universe: 1,
                start: 1,
                end: 3,
            },
            PatchSpan {
                owner: "b".into(),
                detail: "".into(),
                universe: 2,
                start: 1,
                end: 3,
            },
        ];
        assert!(find_overlaps(&spans).is_empty());
    }

    #[test]
    fn overlap_same_universe() {
        let spans = vec![
            PatchSpan {
                owner: "a".into(),
                detail: "".into(),
                universe: 1,
                start: 1,
                end: 6,
            },
            PatchSpan {
                owner: "b".into(),
                detail: "".into(),
                universe: 1,
                start: 4,
                end: 10,
            },
        ];
        let o = &find_overlaps(&spans);
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].start, 4);
        assert_eq!(o[0].end, 6);
    }

    #[test]
    fn segment_spans_wraps_universe() {
        // 200 RGB fixtures = 600 channels, starting at channel 400 universe 1.
        // Universe 1: 400..512 = 113 channels = 37 fixtures + 2 leftover channels.
        // Wait, footprint=3, start=400. Fixtures at 400,403,...,511 -> (511-400)/3 + 1 = 38 fixtures in univ 1 (400..511).
        // Fixture 39 starts at 1 in univ 2.
        let spans = segment_spans("out", "seg", 1, 400, 3, 200);
        assert_eq!(spans[0].universe, 1);
        assert_eq!(spans[0].start, 400);
        // 37 fixtures * 3 = 111 channels -> 400..510.
        assert_eq!(spans[0].end, 510);
        assert_eq!(spans[1].universe, 2);
        assert_eq!(spans[1].start, 1);
        // 200 - 37 = 163 fixtures * 3 = 489 channels -> 1..489.
        assert_eq!(spans[1].end, 489);
    }
}
