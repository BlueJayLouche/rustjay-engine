//! Queue-fed output stream — mono samples pushed by the engine thread play
//! out channel 1 of a chosen device; everything else stays silent.
//!
//! Built for LTC generate (the only consumer today), where the signal must
//! **not** pass through the programme mixer: cue fades would gate it and the
//! master limiter would squash the biphase edges. A dedicated stream also
//! lets LTC leave on a different physical device than the programme output.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use crossbeam::queue::ArrayQueue;
use std::sync::Arc;

/// ~2 s of 48 kHz mono — the producer tops up every engine frame (~16 ms).
const FEED_QUEUE_SAMPLES: usize = 96_000;

#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    #[error("could not enumerate output devices: {source}")]
    Enumerate {
        #[source]
        source: cpal::Error,
    },
    #[error("could not read an output device name: {source}")]
    DeviceName {
        #[source]
        source: cpal::Error,
    },
    #[error("no default output device is available")]
    NoDefaultDevice,
    #[error("output device '{0}' was not found")]
    DeviceNotFound(String),
    #[error("output device '{device}' has no default output config: {source}")]
    DefaultConfig {
        device: String,
        #[source]
        source: cpal::Error,
    },
    #[error("output device '{device}' has no supported sample format (F32, I32, I16, or U16)")]
    NoSupportedFormat { device: String },
    #[error("could not open an output stream on '{device}': {source}")]
    BuildStream {
        device: String,
        #[source]
        source: cpal::Error,
    },
    #[error("could not start the output stream on '{device}': {source}")]
    PlayStream {
        device: String,
        #[source]
        source: cpal::Error,
    },
}

/// Drain `queue` into channel 1 of interleaved `output`, zero-filling the
/// rest — including underruns, which downstream LTC gear reads as a dropout
/// (transport stop), never as garbage.
fn drain_into<S>(queue: &ArrayQueue<f32>, output: &mut [S], channels: usize)
where
    S: SizedSample + FromSample<f32>,
{
    for frame in output.chunks_mut(channels) {
        let sample = queue.pop().unwrap_or(0.0);
        frame[0] = S::from_sample(sample);
        for other in &mut frame[1..] {
            *other = S::from_sample(0.0);
        }
    }
}

/// A running queue-fed output stream. Drop to stop playback.
pub struct QueueOutput {
    _stream: cpal::Stream,
    queue: Arc<ArrayQueue<f32>>,
    device_name: String,
    sample_rate: u32,
}

impl QueueOutput {
    /// Open `device_name` (empty = the default output device) at its native
    /// rate and start playing immediately (silence until fed).
    pub fn start(device_name: &str) -> Result<Self, FeedError> {
        let host = cpal::default_host();
        let device = if device_name.is_empty() {
            host.default_output_device()
                .ok_or(FeedError::NoDefaultDevice)?
        } else {
            host.output_devices()
                .map_err(|source| FeedError::Enumerate { source })?
                .find(|d| {
                    d.description()
                        .map(|desc| desc.name() == device_name)
                        .unwrap_or(false)
                })
                .ok_or_else(|| FeedError::DeviceNotFound(device_name.to_string()))?
        };
        let name = device
            .description()
            .map_err(|source| FeedError::DeviceName { source })?
            .name()
            .to_string();
        let config = device
            .default_output_config()
            .map_err(|source| FeedError::DefaultConfig {
                device: name.clone(),
                source,
            })?;
        let sample_rate = config.sample_rate();
        let channels = config.channels() as usize;
        let stream_config = config.config();

        let queue = Arc::new(ArrayQueue::new(FEED_QUEUE_SAMPLES));
        let reader = Arc::clone(&queue);
        let log_name = name.clone();
        let on_error = move |e| log::error!("[feed-out] stream error on '{log_name}': {e}");
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                stream_config,
                move |data: &mut [f32], _| drain_into(&reader, data, channels),
                on_error,
                None,
            ),
            cpal::SampleFormat::I32 => device.build_output_stream(
                stream_config,
                move |data: &mut [i32], _| drain_into(&reader, data, channels),
                on_error,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                stream_config,
                move |data: &mut [i16], _| drain_into(&reader, data, channels),
                on_error,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_output_stream(
                stream_config,
                move |data: &mut [u16], _| drain_into(&reader, data, channels),
                on_error,
                None,
            ),
            _ => return Err(FeedError::NoSupportedFormat { device: name }),
        }
        .map_err(|source| FeedError::BuildStream {
            device: name.clone(),
            source,
        })?;
        stream.play().map_err(|source| FeedError::PlayStream {
            device: name.clone(),
            source,
        })?;
        log::info!("[feed-out] Playing '{name}' at {sample_rate} Hz (channel 1)");
        Ok(Self {
            _stream: stream,
            queue,
            device_name: name,
            sample_rate,
        })
    }

    /// The output stream's native sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Name of the device actually opened.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Queue samples for playback.
    ///
    /// ponytail: on overflow the excess is dropped (the producer tops up
    /// every engine frame; a full queue means the stream is stalled, and LTC
    /// decoders resync within a frame). Upgrade path: drop-oldest ring.
    pub fn push(&self, samples: &[f32]) {
        for &sample in samples {
            let _ = self.queue.push(sample);
        }
    }

    /// Discard queued audio (e.g. after the source position jumped).
    pub fn clear(&self) {
        while self.queue.pop().is_some() {}
    }

    /// Samples currently queued (playback latency above the device buffer).
    pub fn queued(&self) -> usize {
        self.queue.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_writes_channel_one_and_zeroes_the_rest() {
        let queue = ArrayQueue::new(8);
        queue.push(0.5f32).unwrap();
        queue.push(-0.5f32).unwrap();
        let mut output = [9.0f32; 8]; // 4 stereo frames
        drain_into(&queue, &mut output, 2);
        assert_eq!(output, [0.5, 0.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn drain_converts_formats() {
        let queue = ArrayQueue::new(4);
        queue.push(1.0f32).unwrap();
        queue.push(-1.0f32).unwrap();
        let mut out_i16 = [0i16; 2];
        drain_into(&queue, &mut out_i16, 1);
        assert_eq!(out_i16, [i16::MAX, i16::MIN]);
        queue.push(0.5f32).unwrap();
        let mut out_u16 = [0u16; 2];
        drain_into(&queue, &mut out_u16, 1);
        assert_eq!(out_u16[0], 32768 + 16384);
        assert_eq!(out_u16[1], 32768); // underrun zeroes
    }
}
