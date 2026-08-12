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
use cpal::{FromSample, SizedSample, SupportedBufferSize};
use cuepool_core::AudioOutputDriver;
use std::sync::Arc;
use std::time::Duration;

const TARGET_OUTPUTS: u16 = 8;
const TARGET_RATE: u32 = 48_000;
const FALLBACK_MAX_BUFFER_FRAMES: usize = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostChoice {
    Default,
    Asio,
}

fn host_choice(driver: AudioOutputDriver) -> HostChoice {
    match driver {
        AudioOutputDriver::ASIO => HostChoice::Asio,
        AudioOutputDriver::WASAPI
        | AudioOutputDriver::Wave
        | AudioOutputDriver::DirectSound => HostChoice::Default,
    }
}

fn host_for_driver(driver: AudioOutputDriver) -> Result<cpal::Host, AudioError> {
    match host_choice(driver) {
        HostChoice::Default => Ok(cpal::default_host()),
        HostChoice::Asio => asio_host(),
    }
}

#[cfg(all(target_os = "windows", feature = "asio"))]
fn asio_host() -> Result<cpal::Host, AudioError> {
    cpal::host_from_id(cpal::HostId::Asio).map_err(|source| AudioError::HostUnavailable {
        driver: AudioOutputDriver::ASIO.name(),
        source,
    })
}

#[cfg(not(feature = "asio"))]
fn asio_host() -> Result<cpal::Host, AudioError> {
    Err(AudioError::DriverNotCompiled {
        driver: AudioOutputDriver::ASIO.name(),
    })
}

#[cfg(all(feature = "asio", not(target_os = "windows")))]
fn asio_host() -> Result<cpal::Host, AudioError> {
    Err(AudioError::UnsupportedPlatform {
        driver: AudioOutputDriver::ASIO.name(),
        platform: std::env::consts::OS,
    })
}

fn named_output_devices(
    driver: AudioOutputDriver,
    host: &cpal::Host,
) -> Result<Vec<(String, cpal::Device)>, AudioError> {
    host.output_devices()
        .map_err(|source| AudioError::EnumerateDevices {
            driver: driver.name(),
            source,
        })?
        .map(|device| {
            let name = device.name().map_err(|source| AudioError::DeviceName {
                driver: driver.name(),
                source,
            })?;
            Ok((name, device))
        })
        .collect()
}

/// Result of one host-scoped output configuration attempt.
pub struct AudioEngineSetup {
    pub engine: Result<AudioEngine, AudioError>,
    pub device_names: Vec<String>,
    /// Enumeration is best-effort only for the legacy unnamed/default path.
    pub device_list_error: Option<AudioError>,
}

fn available_devices(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    }
}

/// Central audio engine.
pub struct AudioEngine {
    mixer: Arc<Mixer>,
    _stream: cpal::Stream,
    driver: AudioOutputDriver,
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
        Self::new_configured(AudioOutputDriver::default(), "")
    }

    /// Create an engine using exactly the configured driver and device.
    ///
    /// An empty device name selects the default-host output, or the first ASIO
    /// device because CPAL exposes no ASIO default. A non-empty name never
    /// falls back to another device or host.
    pub fn new_configured(
        driver: AudioOutputDriver,
        configured_device: &str,
    ) -> Result<Self, AudioError> {
        let AudioEngineSetup {
            engine,
            device_list_error,
            ..
        } = Self::configure(driver, configured_device);
        if let Some(error) = device_list_error {
            log::warn!("{error}");
        }
        engine
    }

    /// Build the selected host once, returning both its device names and engine result.
    pub fn configure(driver: AudioOutputDriver, configured_device: &str) -> AudioEngineSetup {
        let host = match host_for_driver(driver) {
            Ok(host) => host,
            Err(error) => {
                return AudioEngineSetup {
                    engine: Err(error),
                    device_names: Vec::new(),
                    device_list_error: None,
                };
            }
        };

        // Preserve the existing default-backend path: an empty WASAPI setting
        // asks CPAL directly for the OS default and does not depend on device
        // enumeration succeeding. ASIO has no CPAL default device, so it is
        // deliberately handled by the strict enumeration path below.
        if configured_device.is_empty() && host_choice(driver) == HostChoice::Default {
            let (device_names, device_list_error) = match named_output_devices(driver, &host) {
                Ok(devices) => (devices.into_iter().map(|(name, _)| name).collect(), None),
                Err(error) => (Vec::new(), Some(error)),
            };
            let available = if device_list_error.is_some() {
                "not enumerated (default host)".to_string()
            } else {
                available_devices(&device_names)
            };
            let engine = match host.default_output_device() {
                Some(device) => {
                    // The legacy default path opened unnamed devices before driver selection.
                    let device_name = device.name().unwrap_or_else(|_| "Unknown".into());
                    Self::open(driver, &device, device_name.clone()).map_err(|source| {
                        AudioError::OpenDevice {
                            driver: driver.name(),
                            device: device_name,
                            available,
                            reason: source.to_string(),
                        }
                    })
                }
                None => Err(AudioError::NoOutputDevice {
                    driver: driver.name(),
                    available,
                }),
            };
            return AudioEngineSetup {
                engine,
                device_names,
                device_list_error,
            };
        }

        let devices = match named_output_devices(driver, &host) {
            Ok(devices) => devices,
            Err(error) => {
                return AudioEngineSetup {
                    engine: Err(error),
                    device_names: Vec::new(),
                    device_list_error: None,
                };
            }
        };
        let names: Vec<_> = devices.iter().map(|(name, _)| name.clone()).collect();
        let alternatives = available_devices(&names);

        let selected = if configured_device.is_empty() {
            devices
                .into_iter()
                .next()
                .ok_or_else(|| AudioError::NoOutputDevice {
                    driver: driver.name(),
                    available: alternatives.clone(),
                })
        } else {
            devices
                .into_iter()
                .find(|(name, _)| name == configured_device)
                .ok_or_else(|| AudioError::DeviceNotFound {
                    driver: driver.name(),
                    device: configured_device.to_string(),
                    available: alternatives.clone(),
                })
        };

        let engine = match selected {
            Ok((device_name, device)) => Self::open(driver, &device, device_name.clone()).map_err(
                |source| AudioError::OpenDevice {
                    driver: driver.name(),
                    device: device_name,
                    available: alternatives,
                    reason: source.to_string(),
                },
            ),
            Err(error) => Err(error),
        };

        AudioEngineSetup {
            engine,
            device_names: names,
            device_list_error: None,
        }
    }

    fn open(
        driver: AudioOutputDriver,
        device: &cpal::Device,
        device_name: String,
    ) -> Result<Self, AudioError> {
        let all_configs: Vec<_> = device.supported_output_configs()?.collect();
        // Prefer F32, then the integer formats exposed by CPAL's ASIO backend.
        // Within that format choose eight channels, then a larger configuration
        // before falling back below eight, and 48 kHz. Dante can expose I16/I32
        // depending on its ASIO encoding.
        let config = select_output_config(&all_configs)
            .ok_or_else(|| AudioError::NoSupportedFormat {
                available: all_configs
                    .iter()
                    .map(|config| config.sample_format().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            })?;

        let sample_format = config.sample_format();
        let sample_rate = TARGET_RATE.clamp(config.min_sample_rate().0, config.max_sample_rate().0);
        let buffer_size = cpal::BufferSize::Default;
        let channels = config.channels();
        let scratch_samples = if sample_format == cpal::SampleFormat::F32 {
            0
        } else {
            max_callback_samples(&config)
        };

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

        let mut scratch = vec![0.0; scratch_samples];
        let stream = device.build_output_stream_raw(
            &config,
            sample_format,
            move |data: &mut cpal::Data, _info: &cpal::OutputCallbackInfo| {
                match sample_format {
                    cpal::SampleFormat::F32 => render_master(
                        data.as_slice_mut::<f32>().expect("CPAL F32 output buffer"),
                        &mixer_clone,
                        &limiter_thresh_clone,
                        &limiter_clone,
                        &metering_clone,
                    ),
                    cpal::SampleFormat::I32 => render_converted::<i32>(
                        data,
                        &mut scratch,
                        &mixer_clone,
                        &limiter_thresh_clone,
                        &limiter_clone,
                        &metering_clone,
                    ),
                    cpal::SampleFormat::I16 => render_converted::<i16>(
                        data,
                        &mut scratch,
                        &mixer_clone,
                        &limiter_thresh_clone,
                        &limiter_clone,
                        &metering_clone,
                    ),
                    _ => data.bytes_mut().fill(0),
                }
            },
            move |err| {
                log::error!("Audio stream error: {}", err);
            },
            None,
        )?;

        stream.play()?;

        log::info!(
            "Audio engine started: {} / {} @ {} Hz, {} channels",
            driver,
            device_name,
            sample_rate,
            channels
        );

        Ok(Self {
            mixer,
            _stream: stream,
            driver,
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

    pub fn driver(&self) -> AudioOutputDriver {
        self.driver
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

        // Up-mix mono to stereo (cues are always stereo; the mixer routes them
        // into the N-channel output).
        if source.channels() == 1 {
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
        _eq_settings: cuepool_core::EQSettings,
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

        if chain.channels() == 1 {
            chain = Box::new(MonoToStereo::new(chain));
        }

        chain
    }

    /// List output devices from exactly the configured driver host.
    pub fn list_devices(
        driver: AudioOutputDriver,
    ) -> Result<Vec<(String, cpal::Device)>, AudioError> {
        let host = host_for_driver(driver)?;
        named_output_devices(driver, &host)
    }
}

fn sample_format_rank(format: cpal::SampleFormat) -> Option<u8> {
    match format {
        cpal::SampleFormat::F32 => Some(0),
        cpal::SampleFormat::I32 => Some(1),
        cpal::SampleFormat::I16 => Some(2),
        _ => None,
    }
}

fn config_preference(config: &cpal::SupportedStreamConfigRange) -> Option<(u8, (bool, u16), u64)> {
    let rate_distance = if (config.min_sample_rate().0..=config.max_sample_rate().0)
        .contains(&TARGET_RATE)
    {
        0
    } else {
        (config.min_sample_rate().0 as i64 - TARGET_RATE as i64).unsigned_abs() + 1
    };
    Some((
        sample_format_rank(config.sample_format())?,
        channel_preference(config.channels()),
        rate_distance,
    ))
}

fn select_output_config(
    configs: &[cpal::SupportedStreamConfigRange],
) -> Option<cpal::SupportedStreamConfigRange> {
    configs
        .iter()
        .filter_map(|config| config_preference(config).map(|key| (key, *config)))
        .min_by_key(|(key, _)| *key)
        .map(|(_, config)| config)
}

fn max_callback_samples(config: &cpal::SupportedStreamConfigRange) -> usize {
    let frames = match config.buffer_size() {
        SupportedBufferSize::Range { max, .. } => *max as usize,
        // ponytail: CPAL exposes no ceiling for some hosts. Oversized callbacks
        // are silenced below; raise this cap if a backend needs larger buffers.
        SupportedBufferSize::Unknown => FALLBACK_MAX_BUFFER_FRAMES,
    };
    frames.saturating_mul(config.channels() as usize)
}

fn channel_preference(channels: u16) -> (bool, u16) {
    (
        channels < TARGET_OUTPUTS,
        channels.abs_diff(TARGET_OUTPUTS),
    )
}

fn render_master(
    data: &mut [f32],
    mixer: &Mixer,
    limiter_threshold: &AtomicF32,
    limiter: &std::sync::Mutex<Limiter>,
    metering: &MeteringProcessor,
) {
    mixer.render(data);
    let threshold = limiter_threshold.load(std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut limiter) = limiter.lock() {
        limiter.threshold = threshold.clamp(0.01, 1.0);
        limiter.process(data);
    }
    metering.analyze(data);
}

fn render_converted<T>(
    data: &mut cpal::Data,
    scratch: &mut [f32],
    mixer: &Mixer,
    limiter_threshold: &AtomicF32,
    limiter: &std::sync::Mutex<Limiter>,
    metering: &MeteringProcessor,
) where
    T: SizedSample + FromSample<f32>,
{
    let output = data
        .as_slice_mut::<T>()
        .expect("CPAL converted output buffer");
    let Some(scratch) = scratch.get_mut(..output.len()) else {
        output.fill(T::EQUILIBRIUM);
        return;
    };
    render_master(scratch, mixer, limiter_threshold, limiter, metering);
    for (output, sample) in output.iter_mut().zip(scratch) {
        *output = T::from_sample(*sample);
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
    #[error("audio output driver {driver} was requested, but CuePool was built without ASIO support; rebuild cuepool with `--features asio`")]
    DriverNotCompiled { driver: &'static str },
    #[error("audio output driver {driver} is only supported on Windows, not {platform}")]
    UnsupportedPlatform {
        driver: &'static str,
        platform: &'static str,
    },
    #[error("audio output driver {driver} is unavailable: {source}")]
    HostUnavailable {
        driver: &'static str,
        #[source]
        source: cpal::HostUnavailable,
    },
    #[error("could not enumerate {driver} output devices: {source}")]
    EnumerateDevices {
        driver: &'static str,
        #[source]
        source: cpal::DevicesError,
    },
    #[error("no {driver} output device is available; available devices: {available}")]
    NoOutputDevice {
        driver: &'static str,
        available: String,
    },
    #[error("configured {driver} output device '{device}' was not found; available devices: {available}")]
    DeviceNotFound {
        driver: &'static str,
        device: String,
        available: String,
    },
    #[error("could not read a {driver} output device name: {source}")]
    DeviceName {
        driver: &'static str,
        #[source]
        source: cpal::DeviceNameError,
    },
    #[error("configured {driver} output device '{device}' could not open: {reason}; available devices: {available}")]
    OpenDevice {
        driver: &'static str,
        device: String,
        available: String,
        reason: String,
    },
    #[error("no supported output sample format (F32, I32, or I16); device formats: {available}")]
    NoSupportedFormat { available: String },
    #[error("cpal error: {0}")]
    Cpal(#[from] cpal::BuildStreamError),
    #[error("cpal supported configs error: {0}")]
    SupportedConfigs(#[from] cpal::SupportedStreamConfigsError),
    #[error("cpal play error: {0}")]
    Play(#[from] cpal::PlayStreamError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_devices() {
        match AudioEngine::list_devices(AudioOutputDriver::default()) {
            Ok(devices) => {
                println!("Found {} output devices", devices.len());
                for (name, _) in devices {
                    println!("  - {name}");
                }
            }
            Err(error) => println!("Could not enumerate output devices: {error}"),
        }
    }

    #[test]
    fn driver_selection_decision_table() {
        assert_eq!(host_choice(AudioOutputDriver::WASAPI), HostChoice::Default);
        assert_eq!(host_choice(AudioOutputDriver::Wave), HostChoice::Default);
        assert_eq!(host_choice(AudioOutputDriver::DirectSound), HostChoice::Default);
        assert_eq!(host_choice(AudioOutputDriver::ASIO), HostChoice::Asio);
    }

    #[test]
    fn missing_device_error_names_driver_device_and_alternatives() {
        let error = AudioError::DeviceNotFound {
            driver: AudioOutputDriver::ASIO.name(),
            device: "Dante Virtual Soundcard (x64)".to_string(),
            available: available_devices(&[
                "ASIO4ALL v2".to_string(),
                "Focusrite USB ASIO".to_string(),
            ]),
        };
        let message = error.to_string();
        assert!(message.contains("ASIO"));
        assert!(message.contains("Dante Virtual Soundcard (x64)"));
        assert!(message.contains("ASIO4ALL v2, Focusrite USB ASIO"));
    }

    #[test]
    #[cfg(not(feature = "asio"))]
    fn asio_request_explains_missing_feature() {
        let message = asio_host().err().unwrap().to_string();
        assert!(message.contains("ASIO"));
        assert!(message.contains("--features asio"));
    }

    #[test]
    #[cfg(all(feature = "asio", not(target_os = "windows")))]
    fn asio_feature_is_a_documented_no_op_off_windows() {
        let message = asio_host().err().unwrap().to_string();
        assert!(message.contains("only supported on Windows"));
    }

    #[test]
    fn asio_native_integer_formats_are_supported() {
        assert_eq!(sample_format_rank(cpal::SampleFormat::F32), Some(0));
        assert_eq!(sample_format_rank(cpal::SampleFormat::I32), Some(1));
        assert_eq!(sample_format_rank(cpal::SampleFormat::I16), Some(2));
    }

    #[test]
    fn eight_channels_then_larger_configs_are_preferred() {
        assert!(channel_preference(8) < channel_preference(9));
        assert!(channel_preference(9) < channel_preference(2));
    }

    #[test]
    fn single_pass_format_selection_matches_two_pass_policy() {
        fn config(
            format: cpal::SampleFormat,
            channels: u16,
            min_rate: u32,
            max_rate: u32,
        ) -> cpal::SupportedStreamConfigRange {
            cpal::SupportedStreamConfigRange::new(
                channels,
                cpal::SampleRate(min_rate),
                cpal::SampleRate(max_rate),
                SupportedBufferSize::Range { min: 64, max: 1024 },
                format,
            )
        }

        fn two_pass(
            configs: &[cpal::SupportedStreamConfigRange],
        ) -> Option<cpal::SupportedStreamConfigRange> {
            let format = configs
                .iter()
                .filter_map(|config| {
                    sample_format_rank(config.sample_format())
                        .map(|rank| (rank, config.sample_format()))
                })
                .min_by_key(|(rank, _)| *rank)?
                .1;
            configs
                .iter()
                .filter(|config| config.sample_format() == format)
                .min_by_key(|config| {
                    let rate_distance = if (config.min_sample_rate().0
                        ..=config.max_sample_rate().0)
                        .contains(&TARGET_RATE)
                    {
                        0
                    } else {
                        (config.min_sample_rate().0 as i64 - TARGET_RATE as i64).unsigned_abs() + 1
                    };
                    (channel_preference(config.channels()), rate_distance)
                })
                .copied()
        }

        let cases = [
            vec![
                config(cpal::SampleFormat::I32, 8, 48_000, 48_000),
                config(cpal::SampleFormat::F32, 2, 44_100, 44_100),
                config(cpal::SampleFormat::F32, 8, 96_000, 96_000),
            ],
            vec![
                config(cpal::SampleFormat::I16, 2, 48_000, 48_000),
                config(cpal::SampleFormat::I16, 9, 44_100, 96_000),
                config(cpal::SampleFormat::I16, 8, 96_000, 96_000),
            ],
            vec![
                config(cpal::SampleFormat::F64, 8, 48_000, 48_000),
                config(cpal::SampleFormat::U16, 8, 48_000, 48_000),
            ],
        ];

        for configs in cases {
            assert_eq!(select_output_config(&configs), two_pass(&configs));
        }
    }
}
