//! Audio input capture — device enumeration and a lock-free sample hand-off.
//!
//! Added for LTC chase (the only consumer today): the input callback converts
//! the configured channel to f32 and pushes into a bounded queue, and the
//! engine thread drains it each frame. The decoder downstream tolerates the
//! odd dropped sample, so the callback never blocks or allocates.

use crate::host::host_for_driver;
use cpal::FromSample;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam::queue::ArrayQueue;
use cuepool_core::AudioOutputDriver;
use std::sync::Arc;

/// ~2 s of 48 kHz mono. The consumer drains every engine frame (~16 ms), so
/// this only fills if the engine loop stalls.
const CAPTURE_QUEUE_SAMPLES: usize = 96_000;

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    // ponytail: stringified — keeps `crate::host` crate-private; the Display
    // text already carries the driver and the underlying cpal error.
    #[error("{0}")]
    Host(String),
    #[error("could not enumerate input devices: {source}")]
    Enumerate {
        #[source]
        source: cpal::Error,
    },
    #[error("could not read an input device name: {source}")]
    DeviceName {
        #[source]
        source: cpal::Error,
    },
    #[error("no default input device is available")]
    NoDefaultDevice,
    #[error("input device '{0}' was not found")]
    DeviceNotFound(String),
    #[error("input device '{device}' has no default input config: {source}")]
    DefaultConfig {
        device: String,
        #[source]
        source: cpal::Error,
    },
    #[error("input device '{device}' has no supported sample format (F32, I16, or U16)")]
    NoSupportedFormat { device: String },
    #[error("could not open an input stream on '{device}': {source}")]
    BuildStream {
        device: String,
        #[source]
        source: cpal::Error,
    },
    #[error("could not start the input stream on '{device}': {source}")]
    PlayStream {
        device: String,
        #[source]
        source: cpal::Error,
    },
}

/// Names of the available audio input devices on the selected driver's host.
pub fn list_input_devices(driver: AudioOutputDriver) -> Result<Vec<String>, InputError> {
    host_for_driver(driver)
        .map_err(|e| InputError::Host(e.to_string()))?
        .input_devices()
        .map_err(|source| InputError::Enumerate { source })?
        .map(|device| {
            device
                .description()
                .map(|d| d.name().to_string())
                .map_err(|source| InputError::DeviceName { source })
        })
        .collect()
}

/// Push channel `channel` (0-based index into `channels`) of interleaved
/// `data` onto `queue` as f32.
///
/// ponytail: on overflow the sample is dropped (the queue holds ~2 s and is
/// drained every engine frame; a fuller queue means a stalled consumer, and
/// the LTC decoder resyncs within a frame). Upgrade path: an owned ring with
/// drop-oldest semantics.
fn push_mono<S>(queue: &ArrayQueue<f32>, data: &[S], channels: usize, channel: usize)
where
    S: cpal::SizedSample,
    f32: FromSample<S>,
{
    for sample in data.iter().skip(channel).step_by(channels) {
        let _ = queue.push(sample.to_sample::<f32>());
    }
}

/// A running input capture stream. Drop to stop capturing.
pub struct InputCapture {
    _stream: cpal::Stream,
    queue: Arc<ArrayQueue<f32>>,
    device_name: String,
    sample_rate: u32,
}

impl InputCapture {
    /// Open `device_name` (empty = the default input device) on `driver`'s
    /// host and start capturing `channel` (1-based, clamped to the device's
    /// channel count) immediately.
    pub fn start(
        driver: AudioOutputDriver,
        device_name: &str,
        channel: u16,
    ) -> Result<Self, InputError> {
        let host = host_for_driver(driver).map_err(|e| InputError::Host(e.to_string()))?;
        let device = if device_name.is_empty() {
            host.default_input_device()
                .ok_or(InputError::NoDefaultDevice)?
        } else {
            host.input_devices()
                .map_err(|source| InputError::Enumerate { source })?
                .find(|d| {
                    d.description()
                        .map(|desc| desc.name() == device_name)
                        .unwrap_or(false)
                })
                .ok_or_else(|| InputError::DeviceNotFound(device_name.to_string()))?
        };
        let name = device
            .description()
            .map_err(|source| InputError::DeviceName { source })?
            .name()
            .to_string();
        let config = device
            .default_input_config()
            .map_err(|source| InputError::DefaultConfig {
                device: name.clone(),
                source,
            })?;
        let sample_rate = config.sample_rate();
        let channels = config.channels() as usize;
        let channel = (channel.max(1) as usize - 1).min(channels - 1);
        let stream_config = config.config();

        let queue = Arc::new(ArrayQueue::new(CAPTURE_QUEUE_SAMPLES));
        let writer = Arc::clone(&queue);
        let log_name = name.clone();
        let on_error = move |e| log::error!("[audio-in] stream error on '{log_name}': {e}");
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                stream_config,
                move |data: &[f32], _| push_mono(&writer, data, channels, channel),
                on_error,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                stream_config,
                move |data: &[i16], _| push_mono(&writer, data, channels, channel),
                on_error,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                stream_config,
                move |data: &[u16], _| push_mono(&writer, data, channels, channel),
                on_error,
                None,
            ),
            _ => return Err(InputError::NoSupportedFormat { device: name }),
        }
        .map_err(|source| InputError::BuildStream {
            device: name.clone(),
            source,
        })?;
        stream.play().map_err(|source| InputError::PlayStream {
            device: name.clone(),
            source,
        })?;
        log::info!(
            "[audio-in] Capturing '{name}' at {sample_rate} Hz, {channels} ch (ch {} used)",
            channel + 1
        );
        Ok(Self {
            _stream: stream,
            queue,
            device_name: name,
            sample_rate,
        })
    }

    /// The input stream's native sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Name of the device actually opened.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Append all buffered samples to `out`, emptying the queue.
    pub fn drain_into(&self, out: &mut Vec<f32>) {
        while let Some(sample) = self.queue.pop() {
            out.push(sample);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_mono_converts_formats_and_takes_the_selected_channel() {
        let queue = ArrayQueue::new(16);
        push_mono(&queue, &[0.0f32, 9.0, -1.0, 9.0, 0.5, 9.0], 2, 0);
        assert_eq!(queue.pop(), Some(0.0));
        assert_eq!(queue.pop(), Some(-1.0));
        assert_eq!(queue.pop(), Some(0.5));
        assert!(queue.is_empty());

        push_mono(&queue, &[i16::MAX, 0, i16::MIN, 0], 2, 0);
        assert!((queue.pop().unwrap() - 1.0).abs() < 1e-4);
        assert!((queue.pop().unwrap() - -1.0).abs() < 1e-4);

        push_mono(&queue, &[u16::MAX, 0, u16::MIN, 0, 32768, 0], 2, 0);
        assert!((queue.pop().unwrap() - 1.0).abs() < 1e-4);
        assert!((queue.pop().unwrap() - -1.0).abs() < 1e-4);
        assert!(queue.pop().unwrap().abs() < 1e-4);
    }

    #[test]
    fn push_mono_reads_later_channels() {
        let queue = ArrayQueue::new(16);
        push_mono(&queue, &[0.0f32, 1.0, 2.0, 3.0, 4.0, 5.0], 3, 2);
        assert_eq!(queue.pop(), Some(2.0));
        assert_eq!(queue.pop(), Some(5.0));
        assert!(queue.is_empty());
    }

    #[test]
    fn push_mono_drops_on_overflow_instead_of_blocking() {
        let queue = ArrayQueue::new(2);
        push_mono(&queue, &[1.0f32, 2.0, 3.0], 1, 0);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.pop(), Some(1.0));
        assert_eq!(queue.pop(), Some(2.0));
    }
}
