//! [`DmxFrame`] — the protocol-neutral currency passed from the mapping layer to
//! a [`crate::DmxTransport`].
//!
//! A frame is a sparse, sorted set of DMX universes. Universes are keyed by a
//! flat `u16` (sACN's 16-bit universe; Art-Net uses the low 15 bits as its
//! PortAddress). Sorting (via [`BTreeMap`]) keeps the on-wire universe order
//! deterministic, which matters for predictable golden-vector tests and for
//! consoles that care about packet ordering within a frame.

use std::collections::BTreeMap;

/// Number of DMX slots (channels) in one universe.
pub const DMX_UNIVERSE_SIZE: usize = 512;

/// One DMX slot buffer (512 channels, start code excluded).
pub type Universe = [u8; DMX_UNIVERSE_SIZE];

/// A sparse, sorted set of universes for a single network tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DmxFrame {
    universes: BTreeMap<u16, Universe>,
}

impl DmxFrame {
    /// Create an empty frame.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mutable access to a universe buffer, inserting a zeroed one if absent.
    ///
    /// This is the workhorse for the patch/packing layer: write fixture bytes
    /// directly into the returned slice.
    pub fn universe_mut(&mut self, universe: u16) -> &mut Universe {
        self.universes
            .entry(universe)
            .or_insert([0u8; DMX_UNIVERSE_SIZE])
    }

    /// Replace a whole universe buffer.
    pub fn set(&mut self, universe: u16, data: Universe) {
        self.universes.insert(universe, data);
    }

    /// Read a universe buffer, if present.
    pub fn get(&self, universe: u16) -> Option<&Universe> {
        self.universes.get(&universe)
    }

    /// Iterate universes in ascending universe order.
    pub fn iter(&self) -> impl Iterator<Item = (u16, &Universe)> {
        self.universes.iter().map(|(u, d)| (*u, d))
    }

    /// Number of populated universes.
    pub fn len(&self) -> usize {
        self.universes.len()
    }

    /// Whether the frame has no universes.
    pub fn is_empty(&self) -> bool {
        self.universes.is_empty()
    }

    /// Drop all universes (retains no allocation guarantees).
    pub fn clear(&mut self) {
        self.universes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universe_mut_zero_inits() {
        let mut f = DmxFrame::new();
        let u = f.universe_mut(1);
        assert_eq!(u.len(), DMX_UNIVERSE_SIZE);
        assert!(u.iter().all(|&b| b == 0));
    }

    #[test]
    fn set_get_roundtrip() {
        let mut f = DmxFrame::new();
        let mut data = [0u8; DMX_UNIVERSE_SIZE];
        data[0] = 7;
        data[511] = 9;
        f.set(5, data);
        assert_eq!(f.get(5), Some(&data));
        assert_eq!(f.get(6), None);
    }

    #[test]
    fn iter_is_sorted_by_universe() {
        let mut f = DmxFrame::new();
        f.universe_mut(10)[0] = 1;
        f.universe_mut(2)[0] = 1;
        f.universe_mut(7)[0] = 1;
        let order: Vec<u16> = f.iter().map(|(u, _)| u).collect();
        assert_eq!(order, vec![2, 7, 10]);
    }

    #[test]
    fn len_and_empty() {
        let mut f = DmxFrame::new();
        assert!(f.is_empty());
        f.universe_mut(1);
        f.universe_mut(1); // same universe, no growth
        f.universe_mut(2);
        assert_eq!(f.len(), 2);
        f.clear();
        assert!(f.is_empty());
    }
}
