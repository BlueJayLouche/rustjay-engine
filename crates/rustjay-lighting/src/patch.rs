//! Universe packing — lay fixtures out into [`DmxFrame`] channels.
//!
//! The mapping layer produces a flat byte buffer of `count × footprint` channel
//! values (fixture-major); [`pack_fixtures`] writes them into a frame starting at
//! a 1-based DMX address, advancing to the next universe when a whole fixture
//! would not fit. Fixtures are never split across a universe boundary, matching
//! pixel-tape / LED-fixture patching convention.

use crate::dmx::{DmxFrame, DMX_UNIVERSE_SIZE};

/// Write fixtures from `data` (fixture-major, `footprint` channels each) into
/// `frame`, beginning at `start_universe` / `start_channel` (1-based).
///
/// A fixture that would overflow the current universe begins at channel 1 of the
/// next universe (`start_universe + 1`, wrapping `u16`). A trailing partial
/// fixture (when `data.len()` is not a multiple of `footprint`) is written as-is.
/// No-op if `footprint` is 0 or larger than a universe.
pub fn pack_fixtures(
    frame: &mut DmxFrame,
    footprint: usize,
    data: &[u8],
    start_universe: u16,
    start_channel: u16,
) {
    if footprint == 0 || footprint > DMX_UNIVERSE_SIZE {
        return;
    }
    let mut universe = start_universe;
    // 0-based channel offset within the current universe.
    let mut ch = start_channel.max(1) as usize - 1;

    for fixture in data.chunks(footprint) {
        if ch + footprint > DMX_UNIVERSE_SIZE {
            universe = universe.wrapping_add(1);
            ch = 0;
        }
        let buf = frame.universe_mut(universe);
        let end = (ch + fixture.len()).min(DMX_UNIVERSE_SIZE);
        buf[ch..end].copy_from_slice(&fixture[..end - ch]);
        ch += footprint;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_two_rgb_fixtures_with_offset() {
        let mut frame = DmxFrame::new();
        // Two fixtures: [10,11,12] and [20,21,22], starting at universe 5 ch 10.
        let data = [10, 11, 12, 20, 21, 22];
        pack_fixtures(&mut frame, 3, &data, 5, 10);

        let u = frame.get(5).expect("universe 5 written");
        // 1-based ch 10 → 0-based index 9.
        assert_eq!(&u[9..15], &[10, 11, 12, 20, 21, 22]);
        // Surrounding channels untouched.
        assert_eq!(u[8], 0);
        assert_eq!(u[15], 0);
        assert_eq!(frame.len(), 1);
    }

    #[test]
    fn fixture_wraps_to_next_universe_without_splitting() {
        let mut frame = DmxFrame::new();
        // 171 RGB fixtures from universe 1 ch 1. 170 fixtures fill ch 1..510;
        // fixture 171 would need ch 511..513 (>512) → universe 2 ch 1.
        let mut data = Vec::new();
        for i in 0u16..171 {
            let v = (i % 256) as u8;
            data.extend_from_slice(&[v, v, v]);
        }
        pack_fixtures(&mut frame, 3, &data, 1, 1);

        let u1 = frame.get(1).expect("universe 1");
        let u2 = frame.get(2).expect("universe 2 (overflow)");
        // Last fully-placed fixture in universe 1 is #170 (value 169) at ch 508..510.
        assert_eq!(&u1[507..510], &[169, 169, 169]);
        // Channels 510,511 (0-based) must be untouched — fixture 171 did not split.
        assert_eq!(u1[510], 0);
        assert_eq!(u1[511], 0);
        // Fixture 171 (value 170) lands at universe 2 ch 1.
        assert_eq!(&u2[0..3], &[170, 170, 170]);
    }

    #[test]
    fn footprint_larger_than_three_pads_through() {
        let mut frame = DmxFrame::new();
        // RGBW-shaped 4-channel fixtures.
        let data = [1, 2, 3, 4, 5, 6, 7, 8];
        pack_fixtures(&mut frame, 4, &data, 1, 1);
        let u = frame.get(1).unwrap();
        assert_eq!(&u[0..8], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn invalid_footprint_is_noop() {
        let mut frame = DmxFrame::new();
        pack_fixtures(&mut frame, 0, &[1, 2, 3], 1, 1);
        pack_fixtures(&mut frame, 600, &[1, 2, 3], 1, 1);
        assert!(frame.is_empty());
    }
}
