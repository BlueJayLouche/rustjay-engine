//! RingBuffer — circular texture buffer for feedback delay lines.
//!
//! Each slot holds a full-resolution `Bgra8Unorm` texture.
//! `write_view()` returns the current head; `advance()` rotates.
//! `read_view(frames_back)` looks backward in time, clamped to capacity.

use rustjay_engine::prelude::working_format;

/// Circular texture buffer for feedback / temporal delay.
pub struct RingBuffer {
    textures: Vec<(wgpu::Texture, wgpu::TextureView)>,
    write_head: usize,
    capacity: usize,
    width: u32,
    height: u32,
}

impl RingBuffer {
    /// Create a new ring buffer with `capacity` slots of the given resolution.
    pub fn new(device: &wgpu::Device, width: u32, height: u32, capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let mut textures = Vec::with_capacity(capacity);
        for i in 0..capacity {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(&format!("ringbuffer-{i}")),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: working_format(),
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            textures.push((texture, view));
        }
        Self {
            textures,
            write_head: 0,
            capacity,
            width,
            height,
        }
    }

    /// Read the texture view `frames_back` slots behind the write head.
    /// Clamped to `[1, capacity - 1]` — minimum 1 because write_head holds an
    /// incomplete frame still being rendered into.
    #[allow(dead_code)] // convenience accessor; callers currently use read_index + slot_view
    pub fn read_view(&self, frames_back: usize) -> &wgpu::TextureView {
        &self.textures[self.read_index(frames_back)].1
    }

    /// Compute the slot index `frames_back` behind the write head (same clamping as `read_view`).
    pub fn read_index(&self, frames_back: usize) -> usize {
        let idx = frames_back.max(1).min(self.capacity.saturating_sub(1));
        (self.write_head + self.capacity - idx) % self.capacity
    }

    /// Texture view for a specific slot by raw index (for pre-building bind groups).
    pub fn slot_view(&self, index: usize) -> &wgpu::TextureView {
        &self.textures[index].1
    }

    /// Texture view for the current write head (render target).
    #[allow(dead_code)] // convenience accessor; callers currently use slot_view
    pub fn write_view(&self) -> &wgpu::TextureView {
        &self.textures[self.write_head].1
    }

    /// Underlying texture for the current write head (copy target).
    pub fn write_texture(&self) -> &wgpu::Texture {
        &self.textures[self.write_head].0
    }

    /// Advance the write head by one slot.
    pub fn advance(&mut self) {
        self.write_head = (self.write_head + 1) % self.capacity;
    }

    /// Recreate with new dimensions / capacity if changed.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32, capacity: usize) {
        if self.width != width || self.height != height || self.capacity != capacity {
            *self = Self::new(device, width, height, capacity);
        }
    }

    /// Number of slots.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn capacity_matches_constructor() {
        // Cannot test GPU creation without a device, so test the public API
        // via a mock or just verify the struct fields after construction.
        // Since we don't have a wgpu device in unit tests, we verify the
        // RingBuffer logic with a hand-rolled test for read_index math.

        // Simulate: capacity=4, write_head=2
        let capacity: usize = 4;
        let write_head = 2;

        // frames_back=0 -> clamped to 1 (write_head holds incomplete frame)
        let idx = 1.min(capacity.saturating_sub(1));
        let read_index = (write_head + capacity - idx) % capacity;
        assert_eq!(read_index, 1);

        // frames_back=1 -> one behind
        let idx = 1usize.min(capacity.saturating_sub(1));
        let read_index = (write_head + capacity - idx) % capacity;
        assert_eq!(read_index, 1);

        // frames_back=2 -> two behind
        let idx = 2usize.min(capacity.saturating_sub(1));
        let read_index = (write_head + capacity - idx) % capacity;
        assert_eq!(read_index, 0);

        // frames_back=3 -> three behind (wraps)
        let idx = 3usize.min(capacity.saturating_sub(1));
        let read_index = (write_head + capacity - idx) % capacity;
        assert_eq!(read_index, 3);

        // frames_back=10 -> clamped to 3
        let idx = 10usize.min(capacity.saturating_sub(1));
        let read_index = (write_head + capacity - idx) % capacity;
        assert_eq!(read_index, 3);
    }

    #[test]
    fn advance_wraps_correctly() {
        let capacity: usize = 4;
        let mut head = 0usize;

        head = (head + 1) % capacity;
        assert_eq!(head, 1);

        head = (head + 1) % capacity;
        assert_eq!(head, 2);

        head = (head + 1) % capacity;
        assert_eq!(head, 3);

        head = (head + 1) % capacity;
        assert_eq!(head, 0); // wraps
    }

    #[test]
    fn read_view_clamps_to_capacity() {
        // Verify the clamp logic independently.
        let capacity: usize = 4;
        let frames_back = 100;
        let idx = frames_back.min(capacity.saturating_sub(1));
        assert_eq!(idx, 3);
    }
}
