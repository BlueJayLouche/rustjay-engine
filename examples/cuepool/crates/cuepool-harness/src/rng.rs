//! Deterministic PRNG. Every fuzz failure must reproduce from its seed, so this
//! is the ONLY randomness in the harness — no `rand`, no system entropy.
//! ponytail: xorshift64* is not cryptographic and not meant to be; it is a
//! reproducible byte source. Upgrade path if distribution quality ever matters:
//! swap in SplitMix64, same interface.

pub struct Xorshift64(u64);

impl Xorshift64 {
    /// Seed must be non-zero; 0 is remapped (xorshift has a fixed point at 0).
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 { 0x9E3779B97F4A7C15 } else { seed })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    pub fn next_byte(&mut self) -> u8 {
        (self.next_u64() >> 56) as u8
    }

    pub fn next_bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next_byte()).collect()
    }

    /// Uniform-ish in `[lo, hi)`. Returns `lo` if the range is empty.
    pub fn next_range(&mut self, lo: u32, hi: u32) -> u32 {
        if hi <= lo { return lo; }
        lo + (self.next_u64() % (hi - lo) as u64) as u32
    }
}

#[test]
fn is_deterministic_and_never_sticks_at_zero() {
    let a: Vec<u64> = (0..8).map(|_| Xorshift64::new(42).next_u64()).collect();
    assert!(a.iter().all(|&v| v == a[0]), "same seed must give same first value");
    let mut r = Xorshift64::new(0);
    assert!((0..1000).all(|_| r.next_u64() != 0), "zero seed must not degenerate");
}
