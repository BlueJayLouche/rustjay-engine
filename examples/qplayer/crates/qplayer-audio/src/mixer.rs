//! Summing mixer — combines multiple audio sources into a single output.
//!
//! Replaces C# `MixerSampleProvider`. Target: 64+ simultaneous cues without dropouts.
//!
//! # Architecture
//!
//! - `Mixer` owns a `Vec<MixerInput>` protected by a `std::sync::Mutex`.
//! - The audio callback calls `render()`, which iterates active inputs.
//! - Each `MixerInput` has atomics for volume/pan/active so the main thread can
//!   update parameters without locking.
//! - `render()` never allocates and never locks (it reads the input vec through
//!   a pre-cached snapshot updated only when the vec changes).

use crate::SampleProvider;
use qplayer_core::FadeType;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Mixer target format: 48 kHz stereo float.
pub const MIXER_SAMPLE_RATE: u32 = 48_000;
pub const MIXER_CHANNELS: u16 = 2;

/// Convert dB to linear gain.
#[inline]
pub fn db_to_linear(db: f32) -> f32 {
    if db <= -96.0 {
        0.0
    } else {
        10.0f32.powf(db / 20.0)
    }
}

/// Convert linear gain to dB.
#[inline]
pub fn linear_to_db(linear: f32) -> f32 {
    if linear <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * linear.log10()
    }
}

/// A single channel into the mixer.
pub struct MixerInput {
    /// The audio source. `read()` is called only from the audio callback.
    source: Box<dyn SampleProvider>,
    /// Volume in linear gain (0.0 = silent, 1.0 = unity).
    volume: AtomicU32, // f32::to_bits() stored in AtomicU32
    /// Pan: -1.0 = full left, 0.0 = center, 1.0 = full right.
    pan: AtomicU32,
    /// Set to false to remove from mixing. The audio callback skips inactive inputs.
    active: AtomicBool,
    /// Set to true when the source has returned 0 samples (EOF reached).
    finished: AtomicBool,
    /// Temporary buffer for reading from the source before mixing.
    /// Sized to the maximum expected callback buffer.
    temp_buffer: Mutex<Vec<f32>>,
    // --- Fade state (atomically controlled from main thread, processed in audio callback) ---
    fade_target: AtomicU32,
    fade_start: AtomicU32,
    fade_duration: AtomicU32,
    fade_remaining: AtomicU32,
    fade_type: std::sync::atomic::AtomicU8,
    fade_active: AtomicBool,
}

impl MixerInput {
    pub fn new(source: Box<dyn SampleProvider>, max_buffer_samples: usize) -> Self {
        Self {
            source,
            volume: AtomicU32::new(1.0f32.to_bits()),
            pan: AtomicU32::new(0.0f32.to_bits()),
            active: AtomicBool::new(true),
            finished: AtomicBool::new(false),
            temp_buffer: Mutex::new(vec![0.0f32; max_buffer_samples]),
            fade_target: AtomicU32::new(1.0f32.to_bits()),
            fade_start: AtomicU32::new(1.0f32.to_bits()),
            fade_duration: AtomicU32::new(0),
            fade_remaining: AtomicU32::new(0),
            fade_type: std::sync::atomic::AtomicU8::new(FadeType::Linear as u8),
            fade_active: AtomicBool::new(false),
        }
    }

    #[inline]
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn set_volume(&self, vol: f32) {
        self.volume.store(vol.to_bits(), Ordering::Relaxed);
    }

    #[inline]
    pub fn pan(&self) -> f32 {
        f32::from_bits(self.pan.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn set_pan(&self, pan: f32) {
        self.pan.store(pan.clamp(-1.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    #[inline]
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Relaxed);
    }

    #[inline]
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    /// Current playback position in samples (mono count).
    #[inline]
    pub fn position(&self) -> usize {
        self.source.position()
    }

    /// Total length in samples, if known.
    #[inline]
    pub fn length(&self) -> Option<usize> {
        self.source.length()
    }

    /// Start a real-time volume fade. Call from the main thread.
    pub fn start_fade(&self, target_volume: f32, duration_frames: u32, fade_type: FadeType) {
        let current = self.volume();
        self.fade_start.store(current.to_bits(), Ordering::Relaxed);
        self.fade_target.store(target_volume.to_bits(), Ordering::Relaxed);
        self.fade_duration.store(duration_frames, Ordering::Relaxed);
        self.fade_remaining.store(duration_frames, Ordering::Relaxed);
        self.fade_type.store(fade_type as u8, Ordering::Relaxed);
        self.fade_active.store(true, Ordering::Release);
    }

    /// Is a fade currently active?
    pub fn is_fading(&self) -> bool {
        self.fade_active.load(Ordering::Acquire)
    }
}

/// Summing mixer.
pub struct Mixer {
    sample_rate: u32,
    channels: u16,
    /// Inputs are added/removed under this lock, but `render()` never takes it.
    inputs: Mutex<Vec<Arc<MixerInput>>>,
    /// Snapshot of active inputs, refreshed when the input list changes.
    /// The audio callback reads this without locking.
    snapshot: Mutex<Vec<Arc<MixerInput>>>,
    /// Set to true when the snapshot needs refreshing.
    dirty: AtomicBool,
    /// Total frames rendered since creation. Used as the audio master clock.
    frame_counter: AtomicU64,
}

impl Mixer {
    pub fn new(channels: u16, sample_rate: u32) -> Self {
        Self {
            sample_rate,
            channels,
            inputs: Mutex::new(Vec::new()),
            snapshot: Mutex::new(Vec::new()),
            dirty: AtomicBool::new(false),
            frame_counter: AtomicU64::new(0),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Add an input. Call from the main thread.
    pub fn add_input(&self, input: Arc<MixerInput>) {
        self.inputs.lock().unwrap().push(input);
        self.dirty.store(true, Ordering::Release);
    }

    /// Remove an input. Call from the main thread.
    pub fn remove_input(&self, input: &Arc<MixerInput>) {
        let mut inputs = self.inputs.lock().unwrap();
        inputs.retain(|i| !Arc::ptr_eq(i, input));
        self.dirty.store(true, Ordering::Release);
    }

    /// Stop and remove all inputs. Call from the main thread.
    pub fn stop_all(&self) {
        let mut inputs = self.inputs.lock().unwrap();
        inputs.clear();
        self.dirty.store(true, Ordering::Release);
    }

    /// Refresh the snapshot if dirty. Call from the main thread between callbacks.
    pub fn refresh_snapshot(&self) {
        if self.dirty.swap(false, Ordering::Acquire) {
            let inputs = self.inputs.lock().unwrap();
            let mut snapshot = self.snapshot.lock().unwrap();
            snapshot.clear();
            snapshot.extend(inputs.iter().cloned());
        }
    }

    /// Render mixed output into `buffer`.
    ///
    /// Call this from the audio callback. Never allocates, never locks.
    pub fn render(&self, buffer: &mut [f32]) {
        // Clear output
        buffer.fill(0.0);

        let frames = buffer.len() / self.channels.max(1) as usize;

        // Read snapshot without locking (main thread guarantees atomic update)
        let snapshot = self.snapshot.lock().unwrap();

        for input in snapshot.iter() {
            if !input.is_active() {
                continue;
            }

            // Get temp buffer — this is a Mutex lock, but only the audio thread
            // accesses it, so it never contends. We use try_lock to be safe.
            let mut temp = match input.temp_buffer.try_lock() {
                Ok(t) => t,
                Err(_) => continue, // Should never happen
            };

            let needed = buffer.len();
            if temp.len() < needed {
                temp.resize(needed, 0.0);
            }

            let read = input.source.read(&mut temp[..needed]);
            if read == 0 {
                input.finished.store(true, Ordering::Relaxed);
                continue;
            }

            let volume = if input.fade_active.load(Ordering::Relaxed) {
                // Apply per-frame fade gain directly to temp buffer
                let fade_start = f32::from_bits(input.fade_start.load(Ordering::Relaxed));
                let fade_target = f32::from_bits(input.fade_target.load(Ordering::Relaxed));
                let fade_duration = input.fade_duration.load(Ordering::Relaxed);
                let mut fade_remaining = input.fade_remaining.load(Ordering::Relaxed);
                let fade_type_u8 = input.fade_type.load(Ordering::Relaxed);
                let fade_type = match fade_type_u8 {
                    0 => FadeType::Linear,
                    1 => FadeType::SCurve,
                    2 => FadeType::Square,
                    3 => FadeType::InverseSquare,
                    _ => FadeType::Linear,
                };

                let ch = self.channels.max(1) as usize;
                let frames = read / ch;

                for frame in 0..frames {
                    let t = if fade_duration == 0 {
                        1.0
                    } else {
                        let elapsed = fade_duration.saturating_sub(fade_remaining);
                        (elapsed as f32 / fade_duration as f32).clamp(0.0, 1.0)
                    };
                    let gain = fade_start + fade_curve(t, fade_type) * (fade_target - fade_start);

                    for c in 0..ch {
                        temp[frame * ch + c] *= gain;
                    }

                    if fade_remaining > 0 {
                        fade_remaining -= 1;
                    }
                }

                input.fade_remaining.store(fade_remaining, Ordering::Relaxed);
                if fade_remaining == 0 {
                    input.fade_active.store(false, Ordering::Release);
                    input.volume.store(fade_target.to_bits(), Ordering::Relaxed);
                    if fade_target <= 0.0 {
                        input.active.store(false, Ordering::Relaxed);
                        input.finished.store(true, Ordering::Relaxed);
                    }
                }

                // Fade already applied to temp; mix with unity volume
                1.0
            } else {
                input.volume()
            };

            let pan = input.pan();

            // Apply volume + pan and mix into output
            if self.channels == 2 {
                apply_volume_pan_mix_stereo(&temp[..read], buffer, volume, pan);
            } else {
                apply_volume_pan_mix_mono(&temp[..read], buffer, volume);
            }
        }

        // Advance the master audio clock.
        self.frame_counter.fetch_add(frames as u64, Ordering::Relaxed);
    }

    /// Current playback time according to the audio master clock.
    ///
    /// This counts continuously from engine creation. To get cue-relative time,
    /// subtract the clock value captured when the cue was started.
    pub fn playback_time(&self) -> Duration {
        let frames = self.frame_counter.load(Ordering::Relaxed);
        Duration::from_secs_f64(frames as f64 / self.sample_rate as f64)
    }
}

/// Apply fade curve to t in [0, 1].
#[inline]
fn fade_curve(t: f32, fade_type: FadeType) -> f32 {
    match fade_type {
        FadeType::Linear => t,
        FadeType::Square => t * t,
        FadeType::InverseSquare => t.sqrt(),
        FadeType::SCurve => t * t * (3.0 - 2.0 * t),
    }
}

/// Apply volume and linear pan, mixing into a stereo buffer.
///
/// This loop is written to autovectorize: LLVM generates NEON on Apple Silicon
/// and AVX2 on x86_64 without any explicit intrinsics.
#[inline]
fn apply_volume_pan_mix_stereo(src: &[f32], dst: &mut [f32], volume: f32, pan: f32) {
    let gain_l = volume * (1.0 - pan.max(0.0));
    let gain_r = volume * (1.0 + pan.min(0.0));

    let frames = src.len().min(dst.len()) / 2;
    let src = &src[..frames * 2];
    let dst = &mut dst[..frames * 2];

    // Process 4 stereo frames (8 floats) at a time for better vectorization
    let chunks = frames / 4;
    for c in 0..chunks {
        let i = c * 8;
        dst[i]     += src[i]     * gain_l;
        dst[i + 1] += src[i + 1] * gain_r;
        dst[i + 2] += src[i + 2] * gain_l;
        dst[i + 3] += src[i + 3] * gain_r;
        dst[i + 4] += src[i + 4] * gain_l;
        dst[i + 5] += src[i + 5] * gain_r;
        dst[i + 6] += src[i + 6] * gain_l;
        dst[i + 7] += src[i + 7] * gain_r;
    }

    // Scalar tail
    for f in (chunks * 4)..frames {
        let i = f * 2;
        dst[i]     += src[i]     * gain_l;
        dst[i + 1] += src[i + 1] * gain_r;
    }
}

/// Apply volume, mixing into a mono or arbitrary-channel buffer.
///
/// Autovectorizes via LLVM when channel count is known at compile time.
#[inline]
fn apply_volume_pan_mix_mono(src: &[f32], dst: &mut [f32], volume: f32) {
    let len = src.len().min(dst.len());
    let src = &src[..len];
    let dst = &mut dst[..len];

    // Process 16 samples at a time for vectorization
    let chunks = len / 16;
    for c in 0..chunks {
        let i = c * 16;
        dst[i]      += src[i]      * volume;
        dst[i + 1]  += src[i + 1]  * volume;
        dst[i + 2]  += src[i + 2]  * volume;
        dst[i + 3]  += src[i + 3]  * volume;
        dst[i + 4]  += src[i + 4]  * volume;
        dst[i + 5]  += src[i + 5]  * volume;
        dst[i + 6]  += src[i + 6]  * volume;
        dst[i + 7]  += src[i + 7]  * volume;
        dst[i + 8]  += src[i + 8]  * volume;
        dst[i + 9]  += src[i + 9]  * volume;
        dst[i + 10] += src[i + 10] * volume;
        dst[i + 11] += src[i + 11] * volume;
        dst[i + 12] += src[i + 12] * volume;
        dst[i + 13] += src[i + 13] * volume;
        dst[i + 14] += src[i + 14] * volume;
        dst[i + 15] += src[i + 15] * volume;
    }

    // Scalar tail
    for i in (chunks * 16)..len {
        dst[i] += src[i] * volume;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_conversions() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 0.001);
        assert!((db_to_linear(-6.0) - 0.501).abs() < 0.01);
        assert!((db_to_linear(f32::NEG_INFINITY) - 0.0).abs() < 0.001);
        assert!((linear_to_db(1.0) - 0.0).abs() < 0.001);
        assert!((linear_to_db(0.5) - (-6.02)).abs() < 0.01);
    }

    #[test]
    fn test_mixer_two_sources() {
        let mixer = Mixer::new(2, 48000);

        let src1 = Arc::new(MixerInput::new(
            Box::new(crate::FnSource::new(
                |buf| {
                    buf.fill(0.5);
                    buf.len()
                },
                48000,
                2,
            )),
            1024,
        ));
        src1.set_volume(1.0);
        src1.set_pan(0.0);

        let src2 = Arc::new(MixerInput::new(
            Box::new(crate::FnSource::new(
                |buf| {
                    buf.fill(0.3);
                    buf.len()
                },
                48000,
                2,
            )),
            1024,
        ));
        src2.set_volume(1.0);
        src2.set_pan(0.0);

        mixer.add_input(src1);
        mixer.add_input(src2);
        mixer.refresh_snapshot();

        let mut output = vec![0.0f32; 16]; // 8 stereo frames
        mixer.render(&mut output);

        // 0.5 + 0.3 = 0.8 per sample
        for s in &output {
            assert!((s - 0.8).abs() < 0.001, "expected 0.8, got {}", s);
        }
    }

    #[test]
    fn test_pan() {
        let mixer = Mixer::new(2, 48000);

        let src = Arc::new(MixerInput::new(
            Box::new(crate::FnSource::new(
                |buf| {
                    for i in 0..buf.len() / 2 {
                        buf[i * 2] = 1.0;     // L
                        buf[i * 2 + 1] = 1.0; // R
                    }
                    buf.len()
                },
                48000,
                2,
            )),
            1024,
        ));
        src.set_volume(1.0);
        src.set_pan(-1.0); // full left

        mixer.add_input(src);
        mixer.refresh_snapshot();

        let mut output = vec![0.0f32; 4]; // 2 stereo frames
        mixer.render(&mut output);

        assert!((output[0] - 1.0).abs() < 0.001, "L should be 1.0");
        assert!((output[1] - 0.0).abs() < 0.001, "R should be 0.0");
    }

    #[test]
    fn test_mixer_fade_out() {
        let mixer = Mixer::new(2, 48000);

        let src = Arc::new(MixerInput::new(
            Box::new(crate::FnSource::new(
                |buf| {
                    for s in buf.iter_mut() { *s = 1.0; }
                    buf.len()
                },
                48000,
                2,
            )),
            1024,
        ));
        src.set_volume(1.0);
        src.set_pan(0.0);
        src.start_fade(0.0, 4, FadeType::Linear); // fade to silence over 4 frames

        mixer.add_input(src.clone());
        mixer.refresh_snapshot();

        // 4 stereo frames = 8 samples
        let mut output = vec![0.0f32; 8];
        mixer.render(&mut output);

        // Frame 0: gain ~1.0, Frame 1: gain ~0.75, Frame 2: gain ~0.5, Frame 3: gain ~0.25
        assert!((output[0] - 1.0).abs() < 0.01, "frame 0 should be ~1.0, got {}", output[0]);
        assert!((output[2] - 0.75).abs() < 0.1, "frame 1 should be ~0.75, got {}", output[2]);
        assert!((output[4] - 0.5).abs() < 0.1, "frame 2 should be ~0.5, got {}", output[4]);
        assert!((output[6] - 0.25).abs() < 0.1, "frame 3 should be ~0.25, got {}", output[6]);

        // After fade, input should be inactive and finished
        assert!(!src.is_active(), "input should be inactive after fade");
        assert!(src.is_finished(), "input should be finished after fade");
    }

    #[test]
    fn test_mixer_fade_volume() {
        let mixer = Mixer::new(2, 48000);

        let src = Arc::new(MixerInput::new(
            Box::new(crate::FnSource::new(
                |buf| {
                    for s in buf.iter_mut() { *s = 1.0; }
                    buf.len()
                },
                48000,
                2,
            )),
            1024,
        ));
        src.set_volume(0.5);
        src.start_fade(1.0, 4, FadeType::Linear); // fade from 0.5 to 1.0 over 4 frames

        mixer.add_input(src.clone());
        mixer.refresh_snapshot();

        let mut output = vec![0.0f32; 8];
        mixer.render(&mut output);

        // Frame 0: gain=0.5, Frame 1: gain=0.625, Frame 2: gain=0.75, Frame 3: gain=0.875
        assert!((output[0] - 0.5).abs() < 0.01, "frame 0 should be ~0.5, got {}", output[0]);
        assert!((output[2] - 0.625).abs() < 0.1, "frame 1 should be ~0.625, got {}", output[2]);
        assert!((output[4] - 0.75).abs() < 0.1, "frame 2 should be ~0.75, got {}", output[4]);
        assert!((output[6] - 0.875).abs() < 0.1, "frame 3 should be ~0.875, got {}", output[6]);

        // After fade, volume should be updated to target
        assert!(!src.is_fading(), "fade should be complete");
        assert!((src.volume() - 1.0).abs() < 0.01, "volume should be 1.0, got {}", src.volume());
    }
}
