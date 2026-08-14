//! Wall-clock-free time. A soak test must simulate an hour in seconds, so
//! elapsed time is derived from samples rendered, never from `Instant::now`.

use std::time::Duration;

pub struct VirtualClock {
    sample_rate: u32,
    block_frames: usize,
    blocks: u64,
}

impl VirtualClock {
    pub fn new(sample_rate: u32, block_frames: usize) -> Self {
        Self {
            sample_rate,
            block_frames,
            blocks: 0,
        }
    }
    pub fn advance(&mut self, blocks: usize) {
        self.blocks += blocks as u64;
    }
    pub fn elapsed(&self) -> Duration {
        Duration::from_secs_f64(
            (self.blocks * self.block_frames as u64) as f64 / self.sample_rate as f64,
        )
    }
}

#[test]
fn one_second_of_blocks_is_one_second() {
    let mut c = VirtualClock::new(48_000, 480);
    c.advance(100);
    assert_eq!(c.elapsed(), Duration::from_secs(1));
}
