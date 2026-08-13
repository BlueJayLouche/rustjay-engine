use cuepool_audio::{
    BufferedSource, LoopProcessor, MIXER_CHANNELS, MIXER_SAMPLE_RATE, Mixer, MixerInput,
    SampleProvider,
};
use cuepool_core::{FadeType, LoopMode};
use cuepool_harness::clock::VirtualClock;
use cuepool_harness::sink::{NullSink, RampSource};
use std::sync::{Arc, Condvar, Mutex};

const BLOCK_FRAMES: usize = 4;

fn seek_harness(len_samples: usize) -> (Arc<MixerInput>, NullSink, VirtualClock) {
    let mixer = Arc::new(Mixer::new(MIXER_CHANNELS, MIXER_SAMPLE_RATE));
    let source = RampSource::new(MIXER_SAMPLE_RATE, MIXER_CHANNELS, len_samples);
    let input = Arc::new(MixerInput::new(Box::new(source), BLOCK_FRAMES * 2));
    mixer.add_input(Arc::clone(&input));
    mixer.refresh_snapshot();
    (
        input,
        NullSink::new(mixer, BLOCK_FRAMES),
        VirtualClock::new(MIXER_SAMPLE_RATE, BLOCK_FRAMES),
    )
}

fn ramp_value(sample: usize) -> f32 {
    (sample % 1000) as f32 / 1000.0 - 0.5
}

struct BlockingSource {
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl SampleProvider for BlockingSource {
    fn read(&self, buffer: &mut [f32]) -> usize {
        let (lock, cvar) = &*self.gate;
        let mut ready = lock.lock().unwrap();
        while !*ready {
            ready = cvar.wait(ready).unwrap();
        }
        buffer.fill(0.25);
        buffer.len()
    }

    fn seek(&self, _sample: usize) {}
    fn position(&self) -> usize {
        0
    }
    fn length(&self) -> Option<usize> {
        Some(200)
    }
    fn sample_rate(&self) -> u32 {
        MIXER_SAMPLE_RATE
    }
    fn channels(&self) -> u16 {
        MIXER_CHANNELS
    }
}

struct SlowSeekSource {
    pos: std::sync::atomic::AtomicUsize,
    seek_gate: Arc<(Mutex<bool>, Condvar)>,
}

impl SampleProvider for SlowSeekSource {
    fn read(&self, buffer: &mut [f32]) -> usize {
        let start = self.pos.fetch_add(buffer.len(), std::sync::atomic::Ordering::Relaxed);
        for (offset, sample) in buffer.iter_mut().enumerate() {
            *sample = ramp_value(start + offset);
        }
        buffer.len()
    }

    fn seek(&self, sample: usize) {
        let (lock, cvar) = &*self.seek_gate;
        let mut released = lock.lock().unwrap();
        while !*released {
            released = cvar.wait(released).unwrap();
        }
        self.pos.store(sample, std::sync::atomic::Ordering::Relaxed);
    }

    fn position(&self) -> usize { self.pos.load(std::sync::atomic::Ordering::Relaxed) }
    fn length(&self) -> Option<usize> { Some(10_000) }
    fn sample_rate(&self) -> u32 { MIXER_SAMPLE_RATE }
    fn channels(&self) -> u16 { MIXER_CHANNELS }
}

#[test]
fn playing_input_seeks_forward_to_new_samples() {
    let (input, mut sink, mut clock) = seek_harness(200);

    assert_eq!(sink.render_block()[0], ramp_value(0));
    clock.advance(1);
    assert_eq!(input.position(), BLOCK_FRAMES * 2);

    input.seek(40);
    assert_eq!(input.position(), 40);
    assert_eq!(sink.render_block()[0], ramp_value(40));
    clock.advance(1);
    assert_eq!(input.position(), 40 + BLOCK_FRAMES * 2);
    assert!((clock.elapsed().as_secs_f64() - 8.0 / MIXER_SAMPLE_RATE as f64).abs() < 1e-9);
}

#[test]
fn playing_input_seeks_backward_to_earlier_samples() {
    let (input, mut sink, _) = seek_harness(200);

    input.seek(80);
    assert_eq!(sink.render_block()[0], ramp_value(80));
    assert_eq!(input.position(), 80 + BLOCK_FRAMES * 2);

    input.seek(16);
    assert_eq!(input.position(), 16);
    assert_eq!(sink.render_block()[0], ramp_value(16));
    assert_eq!(input.position(), 16 + BLOCK_FRAMES * 2);
}

#[test]
fn paused_input_moves_without_rendering_until_resume() {
    let (input, mut sink, mut clock) = seek_harness(200);

    assert_eq!(sink.render_block()[0], ramp_value(0));
    clock.advance(1);
    input.set_active(false);
    input.seek(60);

    assert_eq!(input.position(), 60);
    assert!(sink.render_block().iter().all(|sample| *sample == 0.0));
    clock.advance(1);
    assert_eq!(input.position(), 60);

    input.set_active(true);
    assert_eq!(sink.render_block()[0], ramp_value(60));
    clock.advance(1);
    assert_eq!(input.position(), 60 + BLOCK_FRAMES * 2);
    assert!(clock.elapsed() > std::time::Duration::ZERO);
}

#[test]
fn looping_input_seeks_in_its_loop_relative_timeline() {
    let mixer = Arc::new(Mixer::new(MIXER_CHANNELS, MIXER_SAMPLE_RATE));
    let looped = LoopProcessor::new(Box::new(RampSource::new(
        MIXER_SAMPLE_RATE,
        MIXER_CHANNELS,
        200,
    )));
    looped.set_loop(10, 20, LoopMode::LoopedInfinite, 1);
    let input = Arc::new(MixerInput::new(Box::new(looped), BLOCK_FRAMES * 2));
    mixer.add_input(Arc::clone(&input));
    mixer.refresh_snapshot();
    let mut sink = NullSink::new(mixer, BLOCK_FRAMES);

    // Sample 6 is loop-relative frame 3, so it maps to source frame 13 even
    // before the first render refreshes the processor's cached loop state.
    input.seek(6);
    assert_eq!(input.position(), 6);
    assert_eq!(sink.render_block()[0], ramp_value(26));

    // A request at/past the ten-frame loop is clamped to relative frame 9.
    input.seek(18);
    assert_eq!(sink.render_block()[0], ramp_value(38));
}

#[test]
fn cancelled_tail_fade_restores_its_starting_volume_for_rearm() {
    let (input, mut sink, _) = seek_harness(200);
    input.set_volume(0.8);
    input.start_fade(0.0, 8, FadeType::Linear);

    sink.render_block();
    assert!(input.is_fading());

    input.cancel_fade();
    assert!(!input.is_fading());
    assert!((input.volume() - 0.8).abs() < f32::EPSILON);

    input.seek(40);
    assert!((sink.render_block()[0] - ramp_value(40) * 0.8).abs() < 1e-6);
}

#[test]
fn buffered_source_reports_the_absolute_sought_position() {
    let buffered = BufferedSource::new(Box::new(RampSource::new(
        MIXER_SAMPLE_RATE,
        MIXER_CHANNELS,
        200,
    )));
    let input = MixerInput::new(Box::new(buffered), BLOCK_FRAMES * 2);

    input.seek(40);

    assert_eq!(input.position(), 40);
}

#[test]
fn transient_buffer_refill_does_not_finish_the_input() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let buffered = BufferedSource::new(Box::new(BlockingSource {
        gate: Arc::clone(&gate),
    }));
    let mixer = Arc::new(Mixer::new(MIXER_CHANNELS, MIXER_SAMPLE_RATE));
    let input = Arc::new(MixerInput::new(Box::new(buffered), BLOCK_FRAMES * 2));
    mixer.add_input(Arc::clone(&input));
    mixer.refresh_snapshot();

    let mut sink = NullSink::new(mixer, BLOCK_FRAMES);
    let samples = sink.render_block();
    let (lock, cvar) = &*gate;
    *lock.lock().unwrap() = true;
    cvar.notify_all();

    assert!(samples.iter().all(|sample| *sample == 0.0));
    assert!(!input.is_finished(), "a temporary empty ring is not EOF");
}

#[test]
fn slow_seek_silences_the_old_ring_until_the_new_position_is_ready() {
    let seek_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let buffered = BufferedSource::new(Box::new(SlowSeekSource {
        pos: std::sync::atomic::AtomicUsize::new(0),
        seek_gate: Arc::clone(&seek_gate),
    }));
    let mixer = Arc::new(Mixer::new(MIXER_CHANNELS, MIXER_SAMPLE_RATE));
    let input = Arc::new(MixerInput::new(Box::new(buffered), BLOCK_FRAMES * 2));
    mixer.add_input(Arc::clone(&input));
    mixer.refresh_snapshot();
    let mut sink = NullSink::new(Arc::clone(&mixer), BLOCK_FRAMES);

    for _ in 0..100 {
        if input.position() > 0 { break; }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    input.seek(4_000);

    assert!(
        sink.render_block().iter().all(|sample| *sample == 0.0),
        "a pending seek must not replay samples buffered before the target"
    );

    let (lock, cvar) = &*seek_gate;
    *lock.lock().unwrap() = true;
    cvar.notify_all();
}

#[test]
fn clamped_endpoint_targets_render_the_final_and_first_frames() {
    let (input, mut sink, _) = seek_harness(20);

    // The command clamps a past-EOF request to the start of the final frame.
    input.seek(18);
    assert_eq!(sink.render_block()[0], ramp_value(18));
    assert_eq!(input.position(), 20);

    // Negative and NaN command targets clamp to zero.
    input.seek(0);
    assert_eq!(sink.render_block()[0], ramp_value(0));
}

#[test]
fn time_seek_uses_native_channel_count_and_clamps_to_complete_frames() {
    let source = RampSource::new(10, 6, 120);
    let input = MixerInput::new(Box::new(source), 60);

    assert_eq!(input.seek_seconds(0.5, 120), Some(0.5));
    assert_eq!(input.position(), 30);
    assert_eq!(input.seek_seconds(f64::INFINITY, 120), Some(1.9));
    assert_eq!(input.position(), 114);
    assert_eq!(input.seek_seconds(f64::NAN, 120), Some(0.0));
    assert_eq!(input.position(), 0);
}
