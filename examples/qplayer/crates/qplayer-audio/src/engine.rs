//! Audio engine — device management, output stream lifecycle.
//!
//! Replaces C# `AudioPlaybackManager`. Owns the cpal stream,
//! the master mixer, and all active playback channels.

use crate::buffered_source::BufferedSource;
use crate::channel_converter::MonoToStereo;
use crate::limiter_processor::Limiter;
use crate::metering_processor::{MeterData, MeteringProcessor};
use crate::mixer::{Mixer, MixerInput};
use crate::resampler::ResamplerProcessor;
use crate::SampleProvider;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::Arc;
use std::time::Duration;

/// Central audio engine.
pub struct AudioEngine {
    mixer: Arc<Mixer>,
    _stream: cpal::Stream,
    device_name: String,
    sample_rate: u32,
    channels: u16,
    /// Master limiter threshold (linear gain). 0.95 = -0.45 dBFS.
    /// Shared with the audio callback via Arc so updates are visible immediately.
    limiter_threshold: Arc<AtomicF32>,
    /// Master metering (peak/RMS).
    metering: Arc<MeteringProcessor>,
    /// Master limiter core, shared with the audio callback so GR is readable from main thread.
    limiter: Arc<std::sync::Mutex<Limiter>>,
}

/// Simple atomic f32 using `to_bits`/`from_bits`.
struct AtomicF32 {
    inner: std::sync::atomic::AtomicU32,
}

impl AtomicF32 {
    fn new(v: f32) -> Self {
        Self { inner: std::sync::atomic::AtomicU32::new(v.to_bits()) }
    }
    fn load(&self, ordering: std::sync::atomic::Ordering) -> f32 {
        f32::from_bits(self.inner.load(ordering))
    }
    fn store(&self, v: f32, ordering: std::sync::atomic::Ordering) {
        self.inner.store(v.to_bits(), ordering);
    }
}

impl AudioEngine {
    /// Create an audio engine using the default output device.
    pub fn new_default() -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(AudioError::NoOutputDevice)?;
        Self::new(&device)
    }

    /// Create an audio engine with a specific device.
    pub fn new(device: &cpal::Device) -> Result<Self, AudioError> {
        let all_configs: Vec<_> = device.supported_output_configs()?.collect();
        // Prefer stereo F32 to avoid channel-count mismatches that cause wrong playback speed.
        // Fall back to any F32 config if stereo is unavailable.
        let config = all_configs
            .iter()
            .find(|c| c.sample_format() == cpal::SampleFormat::F32 && c.channels() == 2)
            .or_else(|| {
                all_configs
                    .iter()
                    .find(|c| c.sample_format() == cpal::SampleFormat::F32)
            })
            .ok_or(AudioError::NoF32Format)?
            .clone();

        // Prefer 48kHz stereo, but accept whatever the device supports
        let sample_rate = config.min_sample_rate().0.max(48_000).min(config.max_sample_rate().0);
        let buffer_size = cpal::BufferSize::Default;
        let channels = config.channels();

        let config = cpal::StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size,
        };

        let mixer = Arc::new(Mixer::new(channels, sample_rate));
        let mixer_clone = Arc::clone(&mixer);

        // Master metering — Arc shared so read_meters() on the main thread sees callback writes.
        let metering = Arc::new(MeteringProcessor::new(Box::new(NullSource {
            sample_rate,
            channels,
        })));
        let metering_clone = Arc::clone(&metering);

        // Arc-shared so set_limiter_threshold() on the main thread is visible in the callback.
        let limiter_threshold = Arc::new(AtomicF32::new(0.95));
        let limiter_thresh_clone = Arc::clone(&limiter_threshold);
        // Arc-shared so read_limiter_gr_db() on the main thread reads GR from the callback.
        let limiter = Arc::new(std::sync::Mutex::new(Limiter::new(0.95, sample_rate, channels)));
        let limiter_clone = Arc::clone(&limiter);

        let stream = device.build_output_stream(
            &config,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                mixer_clone.render(data);
                // Master lookahead limiter
                let thresh = limiter_thresh_clone.load(std::sync::atomic::Ordering::Relaxed);
                if let Ok(mut lim) = limiter_clone.lock() {
                    lim.threshold = thresh.clamp(0.01, 1.0);
                    lim.process(data);
                }
                // Master metering: analyze the final mixed+limited output directly.
                metering_clone.analyze(data);
            },
            move |err| {
                log::error!("Audio stream error: {}", err);
            },
            None,
        )?;

        stream.play()?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown".into());
        log::info!(
            "Audio engine started: {} @ {} Hz, {} channels",
            device_name,
            sample_rate,
            channels
        );

        Ok(Self {
            mixer,
            _stream: stream,
            device_name,
            sample_rate,
            channels,
            limiter_threshold,
            metering,
            limiter,
        })
    }

    pub fn mixer(&self) -> &Arc<Mixer> {
        &self.mixer
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Set the master limiter threshold (linear gain, e.g. 0.95).
    pub fn set_limiter_threshold(&self, threshold: f32) {
        self.limiter_threshold.store(threshold.clamp(0.01, 1.0), std::sync::atomic::Ordering::Relaxed);
    }

    /// Read master metering data.
    pub fn read_meters(&self) -> MeterData {
        self.metering.read_meters()
    }

    /// Read current limiter gain reduction in dB (0 = no reduction, negative = active).
    pub fn read_limiter_gr_db(&self) -> f32 {
        if let Ok(lim) = self.limiter.lock() {
            lim.gr_db
        } else {
            0.0
        }
    }

    /// Play a sound by adding it to the mixer.
    ///
    /// Automatically inserts a resampler if the source sample rate differs
    /// from the device rate, and a mono-to-stereo converter if needed.
    pub fn play(&self, source: Box<dyn SampleProvider>) -> Arc<MixerInput> {
        let mut source = source;

        // Resample if needed
        if source.sample_rate() != self.sample_rate {
            source = Box::new(
                ResamplerProcessor::new(source, self.sample_rate)
                    .expect("resampler creation failed — invalid audio parameters?"),
            );
        }

        // Up-mix mono to stereo if needed
        if source.channels() == 1 && self.channels == 2 {
            source = Box::new(MonoToStereo::new(source));
        }

        // Double-buffer the source to decode file I/O on a background thread
        let source = Box::new(BufferedSource::new(source));

        let max_buffer = self.sample_rate as usize * self.channels as usize; // 1 second
        let input = Arc::new(MixerInput::new(source, max_buffer));
        self.mixer.add_input(input.clone());
        input
    }

    /// Refresh the mixer snapshot. Call from the main thread each frame.
    pub fn refresh(&self) {
        self.mixer.refresh_snapshot();
    }

    /// Current playback time of the audio master clock.
    pub fn playback_time(&self) -> Duration {
        self.mixer.playback_time()
    }

    /// Stop all active audio inputs.
    pub fn stop_all(&self) {
        self.mixer.stop_all();
    }

    /// Build a full per-cue processor chain from a decoder.
    ///
    /// Chain: Source → Loop → Resampler → Mono→Stereo → EQ → Fade → Pan → Mixer
    pub fn build_cue_chain(
        &self,
        source: Box<dyn SampleProvider>,
        _eq_settings: qplayer_core::EQSettings,
        _initial_volume: f32,
    ) -> Box<dyn SampleProvider> {
        // TODO: wire LoopProcessor, EqProcessor, FadeProcessor, PanProcessor
        // when the binary crate provides cue parameters.
        // For now, resample and upmix only.
        let mut chain = source;

        if chain.sample_rate() != self.sample_rate {
            chain = Box::new(
                ResamplerProcessor::new(chain, self.sample_rate)
                    .expect("resampler creation failed"),
            );
        }

        if chain.channels() == 1 && self.channels == 2 {
            chain = Box::new(MonoToStereo::new(chain));
        }

        chain
    }

    /// List available output devices.
    pub fn list_devices() -> Vec<(String, cpal::Device)> {
        let host = cpal::default_host();
        host.output_devices()
            .map(|devices| {
                devices
                    .filter_map(|d| d.name().ok().map(|n| (n, d)))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Placeholder source for master metering (metering is driven directly in callback).
struct NullSource {
    sample_rate: u32,
    channels: u16,
}

impl SampleProvider for NullSource {
    fn read(&self, _buffer: &mut [f32]) -> usize { 0 }
    fn seek(&self, _sample: usize) {}
    fn position(&self) -> usize { 0 }
    fn length(&self) -> Option<usize> { None }
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn channels(&self) -> u16 { self.channels }
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no output device available")]
    NoOutputDevice,
    #[error("no F32 sample format supported")]
    NoF32Format,
    #[error("cpal error: {0}")]
    Cpal(#[from] cpal::BuildStreamError),
    #[error("cpal supported configs error: {0}")]
    SupportedConfigs(#[from] cpal::SupportedStreamConfigsError),
    #[error("cpal play error: {0}")]
    Play(#[from] cpal::PlayStreamError),
    #[error("device name error: {0}")]
    DeviceName(#[from] cpal::DeviceNameError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_devices() {
        let devices = AudioEngine::list_devices();
        println!("Found {} output devices", devices.len());
        for (name, _) in &devices {
            println!("  - {}", name);
        }
        // Should find at least one device on any real system
        // (CI might have none)
    }
}
