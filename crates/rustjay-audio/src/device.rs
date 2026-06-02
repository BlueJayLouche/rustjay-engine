//! Audio device enumeration and stream construction.

use crate::fft::{AudioConfig, AudioOutput, process_audio_frame};

use cpal::traits::{DeviceTrait, HostTrait};
use realfft::RealFftPlanner;
use rustfft::num_complex::Complex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// List available audio input devices
pub fn list_audio_devices() -> Vec<String> {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devices) => devices
            .filter_map(|d| d.description().ok().map(|desc| desc.name().to_string()))
            .collect(),
        Err(_) => Vec::new(),
    }
}

pub fn build_stream_f32(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_rate: f32,
    channels: usize,
    fft_size: usize,
    running: Arc<AtomicBool>,
    output: Arc<AudioOutput>,
    audio_config: Arc<AudioConfig>,
    stream_error: Arc<AtomicBool>,
) -> anyhow::Result<cpal::Stream> {
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(fft_size);
    let mut input_buffer: Vec<f32> = Vec::with_capacity(fft_size * 4);
    let mut scratch = r2c.make_scratch_vec();

    // Pre-allocate all per-frame buffers to avoid heap allocs in the callback.
    let mut windowed_buf: Vec<f32> = vec![0.0; fft_size];
    let mut spectrum_buf: Vec<Complex<f32>> = vec![Complex::new(0.0, 0.0); fft_size / 2 + 1];
    let mut magnitudes_buf: Vec<f32> = vec![0.0; fft_size / 2 + 1];
    let mut frame_buf: Vec<f32> = vec![0.0; fft_size];
    let mut mono_buf: Vec<f32> = Vec::with_capacity(fft_size * 2);

    let mut beat_energy = 0.0f32;
    let mut beat_history: VecDeque<f32> = VecDeque::with_capacity(44);
    let mut beat_counter = 0u32;
    let mut norm_peak = 0.01f32;

    let hann_window: Vec<f32> = (0..fft_size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / fft_size as f32).cos()))
        .collect();

    let stream = device.build_input_stream(
        config,
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            if !running.load(Ordering::Acquire) {
                return;
            }
            mono_buf.clear();
            mono_buf.extend(data.chunks(channels).map(|chunk| chunk.iter().sum::<f32>() / channels as f32));
            input_buffer.extend_from_slice(&mono_buf);
            while input_buffer.len() >= fft_size {
                frame_buf.clear();
                frame_buf.extend(input_buffer.drain(..fft_size));
                process_audio_frame(
                    &frame_buf, sample_rate, &hann_window, &r2c, &mut scratch,
                    &mut windowed_buf, &mut spectrum_buf, &mut magnitudes_buf,
                    &mut beat_energy, &mut beat_history, &mut beat_counter,
                    &mut norm_peak,
                    &output, &audio_config,
                );
            }
        },
        move |err| {
            log::error!("Audio stream error: {}", err);
            stream_error.store(true, Ordering::Relaxed);
        },
        None,
    )?;

    Ok(stream)
}

pub fn build_stream_i16(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_rate: f32,
    channels: usize,
    fft_size: usize,
    running: Arc<AtomicBool>,
    output: Arc<AudioOutput>,
    audio_config: Arc<AudioConfig>,
    stream_error: Arc<AtomicBool>,
) -> anyhow::Result<cpal::Stream> {
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(fft_size);
    let mut input_buffer: Vec<f32> = Vec::with_capacity(fft_size * 4);
    let mut scratch = r2c.make_scratch_vec();

    let mut windowed_buf: Vec<f32> = vec![0.0; fft_size];
    let mut spectrum_buf: Vec<Complex<f32>> = vec![Complex::new(0.0, 0.0); fft_size / 2 + 1];
    let mut magnitudes_buf: Vec<f32> = vec![0.0; fft_size / 2 + 1];
    let mut frame_buf: Vec<f32> = vec![0.0; fft_size];
    let mut mono_buf: Vec<f32> = Vec::with_capacity(fft_size * 2);

    let mut beat_energy = 0.0f32;
    let mut beat_history: VecDeque<f32> = VecDeque::with_capacity(44);
    let mut beat_counter = 0u32;
    let mut norm_peak = 0.01f32;

    let hann_window: Vec<f32> = (0..fft_size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / fft_size as f32).cos()))
        .collect();

    let stream = device.build_input_stream(
        config,
        move |data: &[i16], _: &cpal::InputCallbackInfo| {
            if !running.load(Ordering::Acquire) {
                return;
            }
            mono_buf.clear();
            mono_buf.extend(data.chunks(channels).map(|chunk| {
                let sum: i32 = chunk.iter().map(|&s| s as i32).sum();
                (sum as f32 / channels as f32) / 32768.0
            }));
            input_buffer.extend_from_slice(&mono_buf);
            while input_buffer.len() >= fft_size {
                frame_buf.clear();
                frame_buf.extend(input_buffer.drain(..fft_size));
                process_audio_frame(
                    &frame_buf, sample_rate, &hann_window, &r2c, &mut scratch,
                    &mut windowed_buf, &mut spectrum_buf, &mut magnitudes_buf,
                    &mut beat_energy, &mut beat_history, &mut beat_counter,
                    &mut norm_peak,
                    &output, &audio_config,
                );
            }
        },
        move |err| {
            log::error!("Audio stream error: {}", err);
            stream_error.store(true, Ordering::Relaxed);
        },
        None,
    )?;

    Ok(stream)
}

pub fn build_stream_u16(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_rate: f32,
    channels: usize,
    fft_size: usize,
    running: Arc<AtomicBool>,
    output: Arc<AudioOutput>,
    audio_config: Arc<AudioConfig>,
    stream_error: Arc<AtomicBool>,
) -> anyhow::Result<cpal::Stream> {
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(fft_size);
    let mut input_buffer: Vec<f32> = Vec::with_capacity(fft_size * 4);
    let mut scratch = r2c.make_scratch_vec();

    let mut windowed_buf: Vec<f32> = vec![0.0; fft_size];
    let mut spectrum_buf: Vec<Complex<f32>> = vec![Complex::new(0.0, 0.0); fft_size / 2 + 1];
    let mut magnitudes_buf: Vec<f32> = vec![0.0; fft_size / 2 + 1];
    let mut frame_buf: Vec<f32> = vec![0.0; fft_size];
    let mut mono_buf: Vec<f32> = Vec::with_capacity(fft_size * 2);

    let mut beat_energy = 0.0f32;
    let mut beat_history: VecDeque<f32> = VecDeque::with_capacity(44);
    let mut beat_counter = 0u32;
    let mut norm_peak = 0.01f32;

    let hann_window: Vec<f32> = (0..fft_size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / fft_size as f32).cos()))
        .collect();

    let stream = device.build_input_stream(
        config,
        move |data: &[u16], _: &cpal::InputCallbackInfo| {
            if !running.load(Ordering::Acquire) {
                return;
            }
            mono_buf.clear();
            mono_buf.extend(data.chunks(channels).map(|chunk| {
                let sum: u32 = chunk.iter().map(|&s| s as u32).sum();
                ((sum as f32 / channels as f32) / 32768.0) - 1.0
            }));
            input_buffer.extend_from_slice(&mono_buf);
            while input_buffer.len() >= fft_size {
                frame_buf.clear();
                frame_buf.extend(input_buffer.drain(..fft_size));
                process_audio_frame(
                    &frame_buf, sample_rate, &hann_window, &r2c, &mut scratch,
                    &mut windowed_buf, &mut spectrum_buf, &mut magnitudes_buf,
                    &mut beat_energy, &mut beat_history, &mut beat_counter,
                    &mut norm_peak,
                    &output, &audio_config,
                );
            }
        },
        move |err| {
            log::error!("Audio stream error: {}", err);
            stream_error.store(true, Ordering::Relaxed);
        },
        None,
    )?;

    Ok(stream)
}
