use crate::{FramePixels, VideoFrame};
use std::sync::Mutex;

/// Bounded store of decoded-frame pixel allocations.
pub struct FramePool {
    buffers: Mutex<Vec<Vec<u8>>>,
    max_buffers: usize,
}

impl FramePool {
    pub fn new(max_buffers: usize) -> Self {
        Self {
            buffers: Mutex::new(Vec::new()),
            max_buffers,
        }
    }

    pub(crate) fn copy_from_slice(&self, data: &[u8]) -> Vec<u8> {
        let mut buffers = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        // Linear best-fit scan is fine for the small retained bound; switch to
        // size bins if the bound ever grows.
        let buffer_index = buffers
            .iter()
            .enumerate()
            .filter(|(_, buffer)| buffer.capacity() >= data.len())
            .min_by_key(|(_, buffer)| buffer.capacity())
            .or_else(|| {
                buffers
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, buffer)| buffer.capacity())
            })
            .map(|(i, _)| i);
        let mut buffer = buffer_index.map_or_else(Vec::new, |i| buffers.swap_remove(i));
        drop(buffers);
        buffer.clear();
        buffer.extend_from_slice(data);
        buffer
    }

    fn recycle(&self, mut buffer: Vec<u8>) {
        buffer.clear();
        let mut buffers = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        if buffers.len() < self.max_buffers {
            buffers.push(buffer);
        }
    }

    /// Return all pixel allocations owned by a decoded frame.
    pub fn recycle_frame(&self, frame: VideoFrame) {
        match frame.pixels {
            FramePixels::Rgba(data) => self.recycle(data),
            FramePixels::YuvPlanar { y, u, v, .. } => {
                self.recycle(y.data);
                self.recycle(u.data);
                self.recycle(v.data);
            }
            FramePixels::Nv12 { y, uv, .. } => {
                self.recycle(y.data);
                self.recycle(uv.data);
            }
            #[cfg(windows)]
            FramePixels::D3d11Nv12(frame) => {
                frame.complete(Err("frame retired before Vulkan submission".into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_when_empty() {
        let pool = FramePool::new(1);
        let buffer = pool.copy_from_slice(&[1, 2, 3]);

        assert_eq!(buffer, [1, 2, 3]);
        assert!(buffer.capacity() >= 3);
    }

    #[test]
    fn reuses_a_buffer_across_sizes() {
        let pool = FramePool::new(1);
        let mut buffer = Vec::with_capacity(16);
        let allocation = buffer.as_ptr();
        buffer.extend_from_slice(&[0; 8]);
        pool.recycle(buffer);

        let small = pool.copy_from_slice(&[1; 4]);
        assert_eq!(small.as_ptr(), allocation);
        pool.recycle(small);

        let large = pool.copy_from_slice(&[2; 12]);
        assert_eq!(large.as_ptr(), allocation);
        assert_eq!(large, [2; 12]);
    }

    #[test]
    fn chooses_a_large_enough_buffer_for_each_plane() {
        let pool = FramePool::new(2);
        let large = Vec::with_capacity(16);
        let large_allocation = large.as_ptr();
        pool.recycle(large);
        pool.recycle(Vec::with_capacity(4));

        let buffer = pool.copy_from_slice(&[1; 12]);
        assert_eq!(buffer.as_ptr(), large_allocation);
    }

    #[test]
    fn grows_a_retained_buffer_when_none_is_large_enough() {
        let pool = FramePool::new(2);
        pool.recycle(Vec::with_capacity(2));
        pool.recycle(Vec::with_capacity(4));

        let buffer = pool.copy_from_slice(&[1; 8]);
        assert_eq!(buffer, [1; 8]);
        assert_eq!(pool.buffers.lock().unwrap().len(), 1);
    }

    #[test]
    fn enforces_the_retained_buffer_bound() {
        let pool = FramePool::new(2);
        pool.recycle(Vec::with_capacity(1));
        pool.recycle(Vec::with_capacity(2));
        pool.recycle(Vec::with_capacity(3));

        assert_eq!(pool.buffers.lock().unwrap().len(), 2);
    }
}
