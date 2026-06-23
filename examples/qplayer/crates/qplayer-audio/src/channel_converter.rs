//! Channel format converters.
//!
//! - `MonoToStereo`: duplicates mono source to left+right

use crate::SampleProvider;
use std::cell::UnsafeCell;

/// Duplicates a mono source into stereo output.
pub struct MonoToStereo {
    source: Box<dyn SampleProvider>,
    temp: UnsafeCell<Vec<f32>>,
}

impl MonoToStereo {
    pub fn new(source: Box<dyn SampleProvider>) -> Self {
        assert_eq!(source.channels(), 1, "MonoToStereo requires mono input");
        Self {
            source,
            temp: UnsafeCell::new(Vec::new()),
        }
    }
}

impl SampleProvider for MonoToStereo {
    fn read(&self, buffer: &mut [f32]) -> usize {
        let frames = buffer.len() / 2;
        let temp = unsafe { &mut *self.temp.get() };
        temp.resize(frames, 0.0f32);

        let read = self.source.read(temp);
        let read_frames = read;

        for i in 0..read_frames {
            buffer[i * 2] = temp[i];
            buffer[i * 2 + 1] = temp[i];
        }

        read_frames * 2
    }

    fn seek(&self, sample: usize) {
        self.source.seek(sample / 2);
    }

    fn position(&self) -> usize {
        self.source.position() * 2
    }

    fn length(&self) -> Option<usize> {
        self.source.length().map(|l| l * 2)
    }

    fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }

    fn channels(&self) -> u16 {
        2
    }
}

unsafe impl Send for MonoToStereo {}
unsafe impl Sync for MonoToStereo {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FnSource;

    #[test]
    fn test_mono_to_stereo() {
        let source = Box::new(FnSource::new(
            |buf| {
                for i in 0..buf.len() {
                    buf[i] = 0.5;
                }
                buf.len()
            },
            48000,
            1,
        ));

        let conv = MonoToStereo::new(source);
        assert_eq!(conv.channels(), 2);

        let mut output = vec![0.0f32; 8]; // 4 stereo frames
        let read = conv.read(&mut output);
        assert_eq!(read, 8);

        for i in 0..4 {
            assert_eq!(output[i * 2], 0.5);
            assert_eq!(output[i * 2 + 1], 0.5);
        }
    }
}
