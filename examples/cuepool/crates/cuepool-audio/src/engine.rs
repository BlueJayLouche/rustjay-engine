//! Audio engine — device management, output stream lifecycle.
//!
//! Replaces C# `AudioPlaybackManager`. Owns the cpal stream,
//! the master mixer, and all active playback channels.

use crate::SampleProvider;
use crate::buffered_source::BufferedSource;
use crate::channel_converter::MonoToStereo;
use crate::eq_processor::EqProcessor;
use crate::fade_processor::FadeProcessor;
use crate::host::{HostChoice, HostError, host_choice, host_for_driver};
use crate::limiter_processor::Limiter;
use crate::loop_processor::LoopProcessor;
use crate::metering_processor::{MeterData, MeteringProcessor};
use crate::mixer::{Mixer, MixerInput, RenderCache};
use crate::resampler::ResamplerProcessor;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample, SupportedBufferSize};
use cuepool_core::{AudioOutputDriver, EQSettings, FadeType, LoopMode};
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

const TARGET_OUTPUTS: u16 = 8;
const TARGET_RATE: u32 = 48_000;
const FALLBACK_MAX_BUFFER_FRAMES: usize = 16_384;

/// Keep the engine's output-flavored error wording byte-identical while the
/// host selector itself is driver-neutral (see `crate::host`).
impl From<HostError> for AudioError {
    fn from(error: HostError) -> Self {
        match error {
            HostError::NotCompiled { driver } => AudioError::DriverNotCompiled { driver },
            HostError::UnsupportedPlatform { driver, platform } => {
                AudioError::UnsupportedPlatform { driver, platform }
            }
            HostError::Unavailable { driver, source } => {
                AudioError::HostUnavailable { driver, source }
            }
        }
    }
}

fn named_output_devices(
    driver: AudioOutputDriver,
    host: &cpal::Host,
) -> Result<Vec<OutputDevice>, AudioError> {
    // ponytail: CPAL can omit a device before yielding it here. Recovering
    // those devices requires a backend-specific enumeration fork; only devices
    // CPAL yields can retain their configuration-probe failure below.
    host.output_devices()
        .map_err(|source| AudioError::EnumerateDevices {
            driver: driver.name(),
            source,
        })?
        .map(|device| {
            let name = device
                .description()
                .map_err(|source| AudioError::DeviceName {
                    driver: driver.name(),
                    source,
                })?
                .name()
                .to_string();
            let configs = device
                .supported_output_configs()
                .map(|configs| configs.collect());
            let info = classify_device(name, &configs);
            Ok(OutputDevice {
                info,
                device,
                configs,
            })
        })
        .collect()
}

/// A CPAL output device and any failure encountered while probing its formats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub probe_error: Option<String>,
}

impl AudioDeviceInfo {
    pub fn is_available(&self) -> bool {
        self.probe_error.is_none()
    }
}

struct OutputDevice {
    info: AudioDeviceInfo,
    device: cpal::Device,
    configs: Result<Vec<cpal::SupportedStreamConfigRange>, cpal::Error>,
}

fn classify_device(
    name: String,
    configs: &Result<Vec<cpal::SupportedStreamConfigRange>, cpal::Error>,
) -> AudioDeviceInfo {
    AudioDeviceInfo {
        name,
        probe_error: configs.as_ref().err().map(ToString::to_string),
    }
}

/// Result of one host-scoped output configuration attempt.
pub struct AudioEngineSetup {
    pub engine: Result<AudioEngine, AudioError>,
    pub devices: Vec<AudioDeviceInfo>,
    /// Enumeration is best-effort only for the legacy unnamed/default path.
    pub device_list_error: Option<AudioError>,
}

const DEVICE_TROUBLESHOOTING: &str = "check driver sample-format settings (e.g. ASIO drivers set to packed 24-bit: set the driver to 16- or 32-bit) and exclusive-mode conflicts";

fn available_devices(devices: &[AudioDeviceInfo]) -> String {
    if devices.is_empty() {
        "none".to_string()
    } else {
        devices
            .iter()
            .map(|device| match &device.probe_error {
                Some(error) => format!("{} (probe failed: {error})", device.name),
                None => device.name.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn device_diagnostics(devices: &[AudioDeviceInfo]) -> String {
    format!("{}; {DEVICE_TROUBLESHOOTING}", available_devices(devices))
}

/// Central audio engine.
pub struct AudioEngine {
    mixer: Arc<Mixer>,
    _stream: Option<cpal::Stream>,
    driver: AudioOutputDriver,
    device_name: String,
    sample_rate: u32,
    channels: u16,
    /// Master limiter threshold (linear gain). 0.95 = -0.45 dBFS.
    /// Shared with the audio callback via Arc so updates are visible immediately.
    limiter_threshold: Arc<AtomicF32>,
    /// Master metering (peak/RMS).
    metering: Arc<MeteringProcessor>,
    /// Latest limiter gain reduction in dB, published by the audio callback.
    /// The callback owns the `Limiter` outright — no lock on the RT thread.
    limiter_gr_db: Arc<AtomicF32>,
    /// Set by the audio callback if CPAL supplies an unexpected sample format.
    format_mismatch: Arc<std::sync::atomic::AtomicBool>,
    /// Ensures the control thread reports the callback format mismatch only once.
    format_mismatch_logged: std::sync::atomic::AtomicBool,
}

/// Source-rate settings for one cue's processor chain.
pub struct CueChainParams {
    pub start_frame: u64,
    pub end_frame: u64,
    pub loop_mode: LoopMode,
    pub loop_count: u32,
    pub eq: Option<EQSettings>,
    pub fade_in_secs: f32,
    pub fade_type: FadeType,
}

/// Handles needed by the caller after a cue starts playing.
pub struct CuePlayback {
    pub input: Arc<MixerInput>,
    pub loop_counter: Option<Arc<AtomicU32>>,
}

/// Simple atomic f32 using `to_bits`/`from_bits`.
struct AtomicF32 {
    inner: std::sync::atomic::AtomicU32,
}

impl AtomicF32 {
    fn new(v: f32) -> Self {
        Self {
            inner: std::sync::atomic::AtomicU32::new(v.to_bits()),
        }
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

    /// Create an audio-device-free engine for `cuepool-harness`.
    #[cfg(feature = "test-harness")]
    #[doc(hidden)]
    pub fn new_headless(channels: u16, sample_rate: u32) -> Self {
        let mixer = Arc::new(Mixer::new(channels, sample_rate));
        Self {
            mixer,
            _stream: None,
            driver: AudioOutputDriver::default(),
            device_name: "headless".to_string(),
            sample_rate,
            channels,
            limiter_threshold: Arc::new(AtomicF32::new(0.95)),
            metering: Arc::new(MeteringProcessor::new(Box::new(NullSource {
                sample_rate,
                channels,
            }))),
            limiter_gr_db: Arc::new(AtomicF32::new(0.0)),
            format_mismatch: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            format_mismatch_logged: std::sync::atomic::AtomicBool::new(false),
        }
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

    /// Build the selected host once, returning both its device diagnostics and engine result.
    pub fn configure(driver: AudioOutputDriver, configured_device: &str) -> AudioEngineSetup {
        let host = match host_for_driver(driver).map_err(AudioError::from) {
            Ok(host) => host,
            Err(error) => {
                return AudioEngineSetup {
                    engine: Err(error),
                    devices: Vec::new(),
                    device_list_error: None,
                };
            }
        };

        // Preserve the existing default-backend path: an empty WASAPI setting
        // asks CPAL directly for the OS default and does not depend on device
        // enumeration succeeding. ASIO has no CPAL default device, so it is
        // deliberately handled by the strict enumeration path below.
        if configured_device.is_empty() && host_choice(driver) == HostChoice::Default {
            let (devices, device_list_error) = match named_output_devices(driver, &host) {
                Ok(devices) => (
                    devices.into_iter().map(|device| device.info).collect(),
                    None,
                ),
                Err(error) => (Vec::new(), Some(error)),
            };
            let available = if device_list_error.is_some() {
                format!("not enumerated (default host); {DEVICE_TROUBLESHOOTING}")
            } else {
                device_diagnostics(&devices)
            };
            let engine = match host.default_output_device() {
                Some(device) => {
                    // The legacy default path opened unnamed devices before driver selection.
                    let device_name = device
                        .description()
                        .map(|description| description.name().to_string())
                        .unwrap_or_else(|_| "Unknown".into());
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
                devices,
                device_list_error,
            };
        }

        let devices = match named_output_devices(driver, &host) {
            Ok(devices) => devices,
            Err(error) => {
                return AudioEngineSetup {
                    engine: Err(error),
                    devices: Vec::new(),
                    device_list_error: None,
                };
            }
        };
        let device_infos: Vec<_> = devices.iter().map(|device| device.info.clone()).collect();
        let alternatives = device_diagnostics(&device_infos);

        let selected = if configured_device.is_empty() {
            devices
                .into_iter()
                .find(|device| device.info.is_available())
                .ok_or_else(|| AudioError::NoOutputDevice {
                    driver: driver.name(),
                    available: alternatives.clone(),
                })
        } else {
            devices
                .into_iter()
                .find(|device| device.info.name == configured_device)
                .ok_or_else(|| AudioError::DeviceNotFound {
                    driver: driver.name(),
                    device: configured_device.to_string(),
                    available: alternatives.clone(),
                })
        };

        let engine = match selected {
            Ok(OutputDevice {
                info,
                device,
                configs,
            }) => match configs {
                Ok(configs) => Self::open_with_configs(driver, &device, info.name.clone(), configs)
                    .map_err(|source| AudioError::OpenDevice {
                        driver: driver.name(),
                        device: info.name,
                        available: alternatives,
                        reason: source.to_string(),
                    }),
                Err(source) => Err(AudioError::OpenDevice {
                    driver: driver.name(),
                    device: info.name,
                    available: alternatives,
                    reason: format!("configuration probe failed: {source}"),
                }),
            },
            Err(error) => Err(error),
        };

        AudioEngineSetup {
            engine,
            devices: device_infos,
            device_list_error: None,
        }
    }

    fn open(
        driver: AudioOutputDriver,
        device: &cpal::Device,
        device_name: String,
    ) -> Result<Self, AudioError> {
        let all_configs: Vec<_> = device
            .supported_output_configs()
            .map_err(AudioError::SupportedConfigs)?
            .collect();
        Self::open_with_configs(driver, device, device_name, all_configs)
    }

    fn open_with_configs(
        driver: AudioOutputDriver,
        device: &cpal::Device,
        device_name: String,
        all_configs: Vec<cpal::SupportedStreamConfigRange>,
    ) -> Result<Self, AudioError> {
        // Prefer F32, then the integer formats exposed by CPAL's ASIO backend.
        // Within that format choose eight channels, then a larger configuration
        // before falling back below eight, and 48 kHz. Dante can expose
        // I16/I24/I32 depending on its ASIO encoding.
        let config =
            select_output_config(&all_configs).ok_or_else(|| AudioError::NoSupportedFormat {
                available: all_configs
                    .iter()
                    .map(|config| config.sample_format().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            })?;

        let sample_format = config.sample_format();
        let sample_rate = TARGET_RATE.clamp(config.min_sample_rate(), config.max_sample_rate());
        let buffer_size = cpal::BufferSize::Default;
        let channels = config.channels();
        let scratch_samples = if sample_format == cpal::SampleFormat::F32 {
            0
        } else {
            max_callback_samples(&config)
        };

        let config = cpal::StreamConfig {
            channels,
            sample_rate,
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
        let limiter_gr_db = Arc::new(AtomicF32::new(0.0));
        let limiter_gr_db_clone = Arc::clone(&limiter_gr_db);
        // Owned by the callback — never shared, never locked.
        let mut limiter = Limiter::new(0.95, sample_rate, channels);
        let mut render_cache = RenderCache::new();
        let format_mismatch = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let format_mismatch_clone = Arc::clone(&format_mismatch);

        let mut scratch = vec![0.0; scratch_samples];
        let mut oversized_callback_logged = false;
        let stream = device.build_output_stream_raw(
            config,
            sample_format,
            move |data: &mut cpal::Data, _info: &cpal::OutputCallbackInfo| {
                render_output(
                    sample_format,
                    data,
                    &mut scratch,
                    &mut oversized_callback_logged,
                    &format_mismatch_clone,
                    &mixer_clone,
                    &limiter_thresh_clone,
                    &mut limiter,
                    &limiter_gr_db_clone,
                    &metering_clone,
                    &mut render_cache,
                );
            },
            move |err| {
                log::error!("Audio stream error: {}", err);
            },
            None,
        );
        let stream = stream.map_err(AudioError::Cpal)?;

        stream.play().map_err(AudioError::Play)?;

        log::info!(
            "Audio engine started: {} / {} @ {} Hz, {} channels",
            driver,
            device_name,
            sample_rate,
            channels
        );

        Ok(Self {
            mixer,
            _stream: Some(stream),
            driver,
            device_name,
            sample_rate,
            channels,
            limiter_threshold,
            metering,
            limiter_gr_db,
            format_mismatch,
            format_mismatch_logged: std::sync::atomic::AtomicBool::new(false),
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
        self.limiter_threshold.store(
            threshold.clamp(0.01, 1.0),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Read master metering data.
    pub fn read_meters(&self) -> MeterData {
        if self
            .format_mismatch
            .load(std::sync::atomic::Ordering::Relaxed)
            && !self
                .format_mismatch_logged
                .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            log::error!("Audio output sample format mismatch; output silenced");
        }
        self.metering.read_meters()
    }

    /// Read current limiter gain reduction in dB (0 = no reduction, negative = active).
    pub fn read_limiter_gr_db(&self) -> f32 {
        self.limiter_gr_db
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Play a sound by adding it to the mixer.
    ///
    /// Automatically inserts a resampler if the source sample rate differs
    /// from the device rate, and a mono-to-stereo converter if needed.
    pub fn play(&self, source: Box<dyn SampleProvider>) -> Result<Arc<MixerInput>, AudioError> {
        let mut source = source;

        // Resample if needed
        if source.sample_rate() != self.sample_rate {
            let source_rate = source.sample_rate();
            source = Box::new(
                ResamplerProcessor::new(source, self.sample_rate).map_err(|e| {
                    AudioError::Resampler {
                        source_rate,
                        target_rate: self.sample_rate,
                        source: e,
                    }
                })?,
            );
        }

        // Up-mix mono to stereo (cues are always stereo; the mixer routes them
        // into the N-channel output).
        if source.channels() == 1 {
            source = Box::new(MonoToStereo::new(source));
        }

        // Real output decodes on a background thread so its callback never
        // blocks. The headless virtual-time sink reads synchronously so its
        // clock cannot outrun decoding.
        let source: Box<dyn SampleProvider> = if self._stream.is_some() {
            Box::new(BufferedSource::new(source))
        } else {
            source
        };

        let max_buffer = self.sample_rate as usize * self.channels as usize; // 1 second
        let input = Arc::new(MixerInput::new(source, max_buffer));
        self.mixer.add_input(input.clone());
        Ok(input)
    }

    /// Play a cue through Loop → EQ → FadeIn at source rate, then resample,
    /// upmix, buffer, and add it to the mixer.
    pub fn play_cue(
        &self,
        source: Box<dyn SampleProvider>,
        params: CueChainParams,
    ) -> Result<CuePlayback, AudioError> {
        let source_rate = source.sample_rate();
        let loop_counter = matches!(
            params.loop_mode,
            LoopMode::Looped | LoopMode::LoopedInfinite
        )
        .then(|| Arc::new(AtomicU32::new(0)));

        let loop_processor = LoopProcessor::new(source);
        loop_processor.set_loop(
            params.start_frame,
            params.end_frame,
            params.loop_mode,
            params.loop_count,
        );
        let loop_processor = if let Some(counter) = &loop_counter {
            loop_processor.with_loop_counter(Arc::clone(counter))
        } else {
            loop_processor
        };
        let mut source: Box<dyn SampleProvider> = Box::new(loop_processor);

        if let Some(mut settings) = params.eq {
            // `Some` means EQ is enabled; forcing this also covers older show files.
            settings.enabled = true;
            source = Box::new(EqProcessor::new(source, settings));
        }

        if params.fade_in_secs > 0.0 {
            let fade = FadeProcessor::new(source, 0.0);
            fade.start_fade(
                1.0,
                (params.fade_in_secs * source_rate as f32) as u32,
                params.fade_type,
            );
            source = Box::new(fade);
        }

        Ok(CuePlayback {
            input: self.play(source)?,
            loop_counter,
        })
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

    /// List output devices from exactly the configured driver host.
    pub fn list_devices(driver: AudioOutputDriver) -> Result<Vec<AudioDeviceInfo>, AudioError> {
        let host = host_for_driver(driver)?;
        named_output_devices(driver, &host)
            .map(|devices| devices.into_iter().map(|device| device.info).collect())
    }
}

fn sample_format_rank(format: cpal::SampleFormat) -> Option<u8> {
    match format {
        cpal::SampleFormat::F32 => Some(0),
        cpal::SampleFormat::I32 => Some(1),
        cpal::SampleFormat::I24 => Some(2),
        cpal::SampleFormat::I16 => Some(3),
        _ => None,
    }
}

fn config_preference(config: &cpal::SupportedStreamConfigRange) -> Option<(u8, (bool, u16), u64)> {
    let rate_distance =
        if (config.min_sample_rate()..=config.max_sample_rate()).contains(&TARGET_RATE) {
            0
        } else {
            (config.min_sample_rate() as i64 - TARGET_RATE as i64).unsigned_abs() + 1
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
    (channels < TARGET_OUTPUTS, channels.abs_diff(TARGET_OUTPUTS))
}

#[allow(clippy::too_many_arguments)]
fn render_output(
    sample_format: cpal::SampleFormat,
    data: &mut cpal::Data,
    scratch: &mut [f32],
    oversized_callback_logged: &mut bool,
    format_mismatch: &std::sync::atomic::AtomicBool,
    mixer: &Mixer,
    limiter_threshold: &AtomicF32,
    limiter: &mut Limiter,
    limiter_gr_db: &AtomicF32,
    metering: &MeteringProcessor,
    cache: &mut RenderCache,
) {
    match sample_format {
        cpal::SampleFormat::F32 => {
            let Some(output) = data.as_slice_mut::<f32>() else {
                data.bytes_mut().fill(0);
                format_mismatch.store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            };
            render_master(
                output,
                mixer,
                limiter_threshold,
                limiter,
                limiter_gr_db,
                metering,
                cache,
            );
        }
        cpal::SampleFormat::I32 => render_converted::<i32>(
            data,
            scratch,
            oversized_callback_logged,
            format_mismatch,
            mixer,
            limiter_threshold,
            limiter,
            limiter_gr_db,
            metering,
            cache,
        ),
        cpal::SampleFormat::I24 => render_converted::<cpal::I24>(
            data,
            scratch,
            oversized_callback_logged,
            format_mismatch,
            mixer,
            limiter_threshold,
            limiter,
            limiter_gr_db,
            metering,
            cache,
        ),
        cpal::SampleFormat::I16 => render_converted::<i16>(
            data,
            scratch,
            oversized_callback_logged,
            format_mismatch,
            mixer,
            limiter_threshold,
            limiter,
            limiter_gr_db,
            metering,
            cache,
        ),
        _ => data.bytes_mut().fill(0),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_master(
    data: &mut [f32],
    mixer: &Mixer,
    limiter_threshold: &AtomicF32,
    limiter: &mut Limiter,
    limiter_gr_db: &AtomicF32,
    metering: &MeteringProcessor,
    cache: &mut RenderCache,
) {
    mixer.render(data, cache);
    limiter.threshold = limiter_threshold
        .load(std::sync::atomic::Ordering::Relaxed)
        .clamp(0.01, 1.0);
    limiter.process(data);
    limiter_gr_db.store(limiter.gr_db, std::sync::atomic::Ordering::Relaxed);
    metering.analyze(data);
}

#[allow(clippy::too_many_arguments)]
fn render_converted<T>(
    data: &mut cpal::Data,
    scratch: &mut [f32],
    oversized_callback_logged: &mut bool,
    format_mismatch: &std::sync::atomic::AtomicBool,
    mixer: &Mixer,
    limiter_threshold: &AtomicF32,
    limiter: &mut Limiter,
    limiter_gr_db: &AtomicF32,
    metering: &MeteringProcessor,
    cache: &mut RenderCache,
) where
    T: SizedSample + FromSample<f32>,
{
    let Some(output) = data.as_slice_mut::<T>() else {
        data.bytes_mut().fill(0);
        format_mismatch.store(true, std::sync::atomic::Ordering::Relaxed);
        return;
    };
    let scratch_len = scratch.len();
    let Some(scratch) = scratch.get_mut(..output.len()) else {
        if !std::mem::replace(oversized_callback_logged, true) {
            log::error!(
                "Audio {format} callback needs {needed} samples, but the conversion scratch buffer holds {scratch_len}; output silenced",
                format = T::FORMAT,
                needed = output.len(),
            );
        }
        output.fill(T::EQUILIBRIUM);
        return;
    };
    render_master(
        scratch,
        mixer,
        limiter_threshold,
        limiter,
        limiter_gr_db,
        metering,
        cache,
    );
    convert_samples(output, scratch);
}

fn convert_samples<T: SizedSample + FromSample<f32>>(output: &mut [T], input: &[f32]) {
    for (output, sample) in output.iter_mut().zip(input) {
        let sample = if T::FORMAT == cpal::SampleFormat::I24 {
            sample.clamp(-1.0, 1.0 - 1.0 / 8_388_608.0)
        } else {
            *sample
        };
        *output = T::from_sample(sample);
    }
}

/// Placeholder source for master metering (metering is driven directly in callback).
struct NullSource {
    sample_rate: u32,
    channels: u16,
}

impl SampleProvider for NullSource {
    fn read(&self, _buffer: &mut [f32]) -> usize {
        0
    }
    fn seek(&self, _sample: usize) {}
    fn position(&self) -> usize {
        0
    }
    fn length(&self) -> Option<usize> {
        None
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn channels(&self) -> u16 {
        self.channels
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("could not create a resampler ({source_rate} Hz -> {target_rate} Hz): {source}")]
    Resampler {
        source_rate: u32,
        target_rate: u32,
        #[source]
        source: rubato::ResamplerConstructionError,
    },
    #[error(
        "audio output driver {driver} was requested, but CuePool was built without ASIO support; rebuild cuepool with `--features asio`"
    )]
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
        source: cpal::Error,
    },
    #[error("could not enumerate {driver} output devices: {source}")]
    EnumerateDevices {
        driver: &'static str,
        #[source]
        source: cpal::Error,
    },
    #[error("no {driver} output device is available; available devices: {available}")]
    NoOutputDevice {
        driver: &'static str,
        available: String,
    },
    #[error(
        "configured {driver} output device '{device}' was not found; available devices: {available}"
    )]
    DeviceNotFound {
        driver: &'static str,
        device: String,
        available: String,
    },
    #[error("could not read a {driver} output device name: {source}")]
    DeviceName {
        driver: &'static str,
        #[source]
        source: cpal::Error,
    },
    #[error(
        "configured {driver} output device '{device}' could not open: {reason}; available devices: {available}"
    )]
    OpenDevice {
        driver: &'static str,
        device: String,
        available: String,
        reason: String,
    },
    #[error(
        "no supported output sample format (F32, I32, I24, or I16); device formats: {available}"
    )]
    NoSupportedFormat { available: String },
    #[error("cpal error: {0}")]
    Cpal(cpal::Error),
    #[error("cpal supported configs error: {0}")]
    SupportedConfigs(cpal::Error),
    #[error("cpal play error: {0}")]
    Play(cpal::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "test-harness")]
    struct SlowSource {
        first_read: std::sync::atomic::AtomicBool,
    }

    #[cfg(feature = "test-harness")]
    impl SampleProvider for SlowSource {
        fn read(&self, buffer: &mut [f32]) -> usize {
            if self
                .first_read
                .swap(false, std::sync::atomic::Ordering::AcqRel)
            {
                std::thread::sleep(Duration::from_millis(50));
                buffer.fill(0.5);
                buffer.len()
            } else {
                0
            }
        }

        fn seek(&self, _sample: usize) {}

        fn position(&self) -> usize {
            0
        }

        fn length(&self) -> Option<usize> {
            Some(16)
        }

        fn sample_rate(&self) -> u32 {
            TARGET_RATE
        }

        fn channels(&self) -> u16 {
            2
        }
    }

    fn render_test_output(
        sample_format: cpal::SampleFormat,
        data: &mut cpal::Data,
        scratch: &mut [f32],
        format_mismatch: &std::sync::atomic::AtomicBool,
    ) {
        let mixer = Mixer::new(2, TARGET_RATE);
        let limiter_threshold = AtomicF32::new(0.95);
        let mut limiter = Limiter::new(0.95, TARGET_RATE, 2);
        let limiter_gr_db = AtomicF32::new(0.0);
        let metering = MeteringProcessor::new(Box::new(NullSource {
            sample_rate: TARGET_RATE,
            channels: 2,
        }));
        render_output(
            sample_format,
            data,
            scratch,
            &mut false,
            format_mismatch,
            &mixer,
            &limiter_threshold,
            &mut limiter,
            &limiter_gr_db,
            &metering,
            &mut RenderCache::new(),
        );
    }

    #[test]
    fn f32_callback_format_mismatch_silences_output_and_sets_flag() {
        let mut backing = vec![1_i16; 8];
        // SAFETY: `backing` is live, aligned I16 storage with `backing.len()` samples.
        let mut data = unsafe {
            cpal::Data::from_parts(
                backing.as_mut_ptr().cast(),
                backing.len(),
                cpal::SampleFormat::I16,
            )
        };
        let format_mismatch = std::sync::atomic::AtomicBool::new(false);

        render_test_output(
            cpal::SampleFormat::F32,
            &mut data,
            &mut [],
            &format_mismatch,
        );

        assert!(data.bytes().iter().all(|byte| *byte == 0));
        assert!(format_mismatch.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn converted_callback_format_mismatch_silences_output_and_sets_flag() {
        let mut backing = vec![1.0_f32; 8];
        // SAFETY: `backing` is live, aligned F32 storage with `backing.len()` samples.
        let mut data = unsafe {
            cpal::Data::from_parts(
                backing.as_mut_ptr().cast(),
                backing.len(),
                cpal::SampleFormat::F32,
            )
        };
        let format_mismatch = std::sync::atomic::AtomicBool::new(false);
        let mut scratch = vec![0.0; backing.len()];

        render_test_output(
            cpal::SampleFormat::I16,
            &mut data,
            &mut scratch,
            &format_mismatch,
        );

        assert!(data.bytes().iter().all(|byte| *byte == 0));
        assert!(format_mismatch.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn matching_f32_callback_renders_without_setting_mismatch_flag() {
        let mut backing = vec![1.0_f32; 8];
        // SAFETY: `backing` is live, aligned F32 storage with `backing.len()` samples.
        let mut data = unsafe {
            cpal::Data::from_parts(
                backing.as_mut_ptr().cast(),
                backing.len(),
                cpal::SampleFormat::F32,
            )
        };
        let format_mismatch = std::sync::atomic::AtomicBool::new(false);

        render_test_output(
            cpal::SampleFormat::F32,
            &mut data,
            &mut [],
            &format_mismatch,
        );

        assert!(!format_mismatch.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    #[cfg(feature = "test-harness")]
    fn headless_engine_reads_source_synchronously() {
        let engine = AudioEngine::new_headless(2, TARGET_RATE);
        engine
            .play(Box::new(SlowSource {
                first_read: std::sync::atomic::AtomicBool::new(true),
            }))
            .unwrap();
        engine.refresh();

        let mut output = [0.0; 16];
        engine.mixer.render(&mut output, &mut RenderCache::new());

        assert!(output.iter().any(|sample| sample.abs() > 0.1));
    }

    #[test]
    fn test_list_devices() {
        match AudioEngine::list_devices(AudioOutputDriver::default()) {
            Ok(devices) => {
                println!("Found {} output devices", devices.len());
                for device in devices {
                    println!("  - {}", device.name);
                }
            }
            Err(error) => println!("Could not enumerate output devices: {error}"),
        }
    }

    #[test]
    fn missing_device_error_names_driver_device_and_alternatives() {
        let devices = [
            AudioDeviceInfo {
                name: "ASIO4ALL v2".to_string(),
                probe_error: None,
            },
            AudioDeviceInfo {
                name: "Focusrite USB ASIO".to_string(),
                probe_error: None,
            },
        ];
        let error = AudioError::DeviceNotFound {
            driver: AudioOutputDriver::ASIO.name(),
            device: "Dante Virtual Soundcard (x64)".to_string(),
            available: device_diagnostics(&devices),
        };
        let message = error.to_string();
        assert!(message.contains("ASIO"));
        assert!(message.contains("Dante Virtual Soundcard (x64)"));
        assert!(message.contains("ASIO4ALL v2, Focusrite USB ASIO"));
        assert!(message.contains("packed 24-bit"));
        assert!(message.contains("16- or 32-bit"));
        assert!(message.contains("exclusive-mode conflicts"));
    }

    #[test]
    fn probe_failure_is_classified_and_formatted_for_empty_selection() {
        let configs: Result<Vec<cpal::SupportedStreamConfigRange>, cpal::Error> =
            Err(cpal::Error::with_message(
                cpal::ErrorKind::UnsupportedConfig,
                "driver rejected its packed sample format",
            ));
        let failed = classify_device("Dante Virtual Soundcard".to_string(), &configs);
        let available = classify_device("Built-in Output".to_string(), &Ok(Vec::new()));

        assert_eq!(
            failed.probe_error.as_deref(),
            Some("driver rejected its packed sample format")
        );
        assert!(!failed.is_available());
        assert!(available.is_available());

        let all_failed_error = AudioError::NoOutputDevice {
            driver: AudioOutputDriver::ASIO.name(),
            available: device_diagnostics(&[failed]),
        };
        let all_failed_message = all_failed_error.to_string();
        assert!(all_failed_message.contains(
            "Dante Virtual Soundcard (probe failed: driver rejected its packed sample format)"
        ));
        let empty_message = AudioError::NoOutputDevice {
            driver: AudioOutputDriver::ASIO.name(),
            available: device_diagnostics(&[]),
        }
        .to_string();
        assert!(empty_message.contains("available devices: none"));

        for message in [all_failed_message, empty_message] {
            assert!(message.contains("packed 24-bit"));
            assert!(message.contains("16- or 32-bit"));
            assert!(message.contains("exclusive-mode conflicts"));
        }
    }

    #[test]
    fn asio_native_integer_formats_are_supported() {
        assert_eq!(sample_format_rank(cpal::SampleFormat::F32), Some(0));
        assert_eq!(sample_format_rank(cpal::SampleFormat::I32), Some(1));
        assert_eq!(sample_format_rank(cpal::SampleFormat::I24), Some(2));
        assert_eq!(sample_format_rank(cpal::SampleFormat::I16), Some(3));
    }

    #[test]
    fn i24_is_selected_over_i16() {
        let configs = [
            cpal::SupportedStreamConfigRange::new(
                8,
                48_000,
                48_000,
                SupportedBufferSize::Range { min: 64, max: 1024 },
                cpal::SampleFormat::I16,
            ),
            cpal::SupportedStreamConfigRange::new(
                2,
                44_100,
                44_100,
                SupportedBufferSize::Range { min: 64, max: 1024 },
                cpal::SampleFormat::I24,
            ),
        ];

        assert_eq!(
            select_output_config(&configs).unwrap().sample_format(),
            cpal::SampleFormat::I24
        );
    }

    #[test]
    fn float_samples_saturate_when_converted_to_cpal_i24() {
        let mut output = [cpal::I24::default(); 5];
        convert_samples(&mut output, &[1.0, 1.5, 2.0, -1.0, -1.5]);
        assert_eq!(
            output.map(cpal::I24::inner),
            [8_388_607, 8_388_607, 8_388_607, -8_388_608, -8_388_608]
        );
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
                min_rate,
                max_rate,
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
                    let rate_distance = if (config.min_sample_rate()..=config.max_sample_rate())
                        .contains(&TARGET_RATE)
                    {
                        0
                    } else {
                        (config.min_sample_rate() as i64 - TARGET_RATE as i64).unsigned_abs() + 1
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
