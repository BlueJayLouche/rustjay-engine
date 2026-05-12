pub mod device;
pub mod fft;
pub mod routing;

pub use device::list_audio_devices;


use crate::device::{build_stream_f32, build_stream_i16, build_stream_u16};
use crate::fft::{AudioConfig, AudioOutput, DEFAULT_FFT_SIZE};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct AudioAnalyzer {
    stream: Option<cpal::Stream>,
    running: Arc<AtomicBool>,
    stream_error: Arc<AtomicBool>,
    output: Arc<AudioOutput>,
    config: Arc<AudioConfig>,
    fft_size: usize,
}

impl AudioAnalyzer {
    pub fn new() -> Self {
        Self {
            stream: None,
            running: Arc::new(AtomicBool::new(false)),
            stream_error: Arc::new(AtomicBool::new(false)),
            output: Arc::new(AudioOutput::new()),
            config: Arc::new(AudioConfig::new()),
            fft_size: DEFAULT_FFT_SIZE,
        }
    }

    pub fn take_stream_error(&self) -> bool {
        self.stream_error.swap(false, Ordering::Relaxed)
    }

    pub fn fft_size(&self) -> usize { self.fft_size }
    pub fn set_fft_size(&mut self, size: usize) { self.fft_size = size; }

    pub fn start(&mut self) -> anyhow::Result<String> {
        self.start_with_device(None)
    }

    pub fn start_with_device(&mut self, device_name: Option<&str>) -> anyhow::Result<String> {
        log::info!("[Audio] start_with_device: {:?}, fft_size: {}", device_name, self.fft_size);
        if self.stream.is_some() {
            self.running.store(false, Ordering::Release);
            self.stream = None;
        }
        self.running = Arc::new(AtomicBool::new(false));
        self.output = Arc::new(AudioOutput::new());
        self.stream_error = Arc::new(AtomicBool::new(false));

        let host = cpal::default_host();

        let device = match device_name {
            Some(name) => host
                .input_devices()?
                .find(|d| d.name().ok().as_deref() == Some(name))
                .ok_or_else(|| anyhow::anyhow!("Audio device '{}' not found", name))?,
            None => host
                .default_input_device()
                .ok_or_else(|| anyhow::anyhow!("No default input device"))?,
        };

        let actual_name = device.name()?;
        let config = device.default_input_config()?;
        let sample_rate = config.sample_rate() as f32;
        let channels = config.channels() as usize;
        let fft_size = self.fft_size;

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => build_stream_f32(
                &device, &config.into(), sample_rate, channels, fft_size,
                Arc::clone(&self.running), Arc::clone(&self.output),
                Arc::clone(&self.config), Arc::clone(&self.stream_error),
            )?,
            cpal::SampleFormat::I16 => build_stream_i16(
                &device, &config.into(), sample_rate, channels, fft_size,
                Arc::clone(&self.running), Arc::clone(&self.output),
                Arc::clone(&self.config), Arc::clone(&self.stream_error),
            )?,
            cpal::SampleFormat::U16 => build_stream_u16(
                &device, &config.into(), sample_rate, channels, fft_size,
                Arc::clone(&self.running), Arc::clone(&self.output),
                Arc::clone(&self.config), Arc::clone(&self.stream_error),
            )?,
            _ => return Err(anyhow::anyhow!("Unsupported sample format")),
        };

        stream.play()?;
        self.stream = Some(stream);
        self.running.store(true, Ordering::Release);
        log::info!("Audio analyzer started (device: {}, fft: {})", actual_name, fft_size);
        Ok(actual_name)
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        self.stream = None;
        self.output.reset();
        log::info!("Audio analyzer stopped");
    }

    pub fn get_fft(&self) -> [f32; 8] {
        std::array::from_fn(|i| f32::from_bits(self.output.fft[i].load(Ordering::Relaxed)))
    }

    pub fn get_volume(&self) -> f32 {
        f32::from_bits(self.output.volume.load(Ordering::Relaxed))
    }

    pub fn is_beat(&self) -> bool {
        self.output.beat.swap(false, Ordering::Relaxed)
    }

    pub fn get_beat_phase(&self) -> f32 {
        f32::from_bits(self.output.beat_phase.load(Ordering::Relaxed))
    }

    pub fn set_amplitude(&self, v: f32)         { self.config.amplitude.store(v.to_bits(), Ordering::Relaxed); }
    pub fn set_smoothing(&self, v: f32)         { self.config.smoothing.store(v.clamp(0.0, 0.99).to_bits(), Ordering::Relaxed); }
    pub fn get_normalize(&self) -> bool         { self.config.normalize.load(Ordering::Relaxed) }
    pub fn set_normalize(&self, v: bool)        { self.config.normalize.store(v, Ordering::Relaxed); }
    pub fn get_pink_noise_shaping(&self) -> bool{ self.config.pink_noise_shaping.load(Ordering::Relaxed) }
    pub fn set_pink_noise_shaping(&self, v: bool){ self.config.pink_noise_shaping.store(v, Ordering::Relaxed); }
}

impl Default for AudioAnalyzer { fn default() -> Self { Self::new() } }

impl Drop for AudioAnalyzer {
    fn drop(&mut self) { self.stop(); }
}
pub use routing::AudioRoutingState;
