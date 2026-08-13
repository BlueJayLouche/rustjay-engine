use cuepool_audio::{
    AudioEngine, CueChainParams, CuePlayback, MIXER_CHANNELS, MIXER_SAMPLE_RATE, SampleProvider,
};
use cuepool_core::{EQFilter, EQFilterOrder, EQSettings, FadeType, LoopMode};
use cuepool_harness::clock::VirtualClock;
use cuepool_harness::sink::NullSink;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestSource {
    samples: Vec<f32>,
    position: AtomicUsize,
    sample_rate: u32,
    channels: u16,
}

impl TestSource {
    fn constant(sample_rate: u32, channels: u16, frames: usize, value: f32) -> Self {
        Self {
            samples: vec![value; frames * channels as usize],
            position: AtomicUsize::new(0),
            sample_rate,
            channels,
        }
    }
}

impl SampleProvider for TestSource {
    fn read(&self, buffer: &mut [f32]) -> usize {
        let start = self.position.load(Ordering::Relaxed);
        let read = buffer.len().min(self.samples.len().saturating_sub(start));
        buffer[..read].copy_from_slice(&self.samples[start..start + read]);
        self.position.store(start + read, Ordering::Relaxed);
        read
    }

    fn seek(&self, sample: usize) {
        self.position
            .store(sample.min(self.samples.len()), Ordering::Relaxed);
    }

    fn position(&self) -> usize {
        self.position.load(Ordering::Relaxed)
    }

    fn length(&self) -> Option<usize> {
        Some(self.samples.len())
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> u16 {
        self.channels
    }
}

struct CueHarness {
    engine: AudioEngine,
    sink: NullSink,
    clock: VirtualClock,
}

impl CueHarness {
    fn new(block_frames: usize) -> Self {
        let engine = AudioEngine::new_headless(MIXER_CHANNELS, MIXER_SAMPLE_RATE);
        let sink = NullSink::new(Arc::clone(engine.mixer()), block_frames);
        Self {
            engine,
            sink,
            clock: VirtualClock::new(MIXER_SAMPLE_RATE, block_frames),
        }
    }

    fn play(&self, source: TestSource, params: CueChainParams) -> CuePlayback {
        let playback = self.engine.play_cue(Box::new(source), params).unwrap();
        self.engine.refresh();
        playback
    }

    fn render_samples(&mut self, playback: &CuePlayback, target_samples: usize) -> Vec<f32> {
        let mut rendered = Vec::with_capacity(target_samples);
        for _ in 0..10_000 {
            let before = playback.input.position();
            let block = self.sink.render_block().to_vec();
            self.clock.advance(1);
            let read = playback.input.position().saturating_sub(before);
            rendered.extend_from_slice(&block[..read.min(block.len())]);
            if rendered.len() >= target_samples {
                rendered.truncate(target_samples);
                return rendered;
            }
            std::thread::yield_now();
        }
        panic!(
            "audio buffer did not produce {target_samples} samples; got {}",
            rendered.len()
        );
    }
}

fn params() -> CueChainParams {
    CueChainParams {
        start_frame: 0,
        end_frame: 0,
        loop_mode: LoopMode::OneShot,
        loop_count: 1,
        eq: None,
        fade_in_secs: 0.0,
        fade_type: FadeType::Linear,
    }
}

#[test]
fn loop_counter_increments_at_loop_boundary() {
    let mut harness = CueHarness::new(4);
    let playback = harness.play(
        TestSource::constant(MIXER_SAMPLE_RATE, 2, 4, 0.5),
        CueChainParams {
            end_frame: 4,
            loop_mode: LoopMode::Looped,
            loop_count: 2,
            ..params()
        },
    );
    let counter = playback.loop_counter.as_ref().unwrap();

    harness.render_samples(&playback, 16);

    assert_eq!(counter.load(Ordering::Relaxed), 1);
}

#[test]
fn fade_in_ramps_from_silence_to_full_volume() {
    let mut harness = CueHarness::new(16);
    let playback = harness.play(
        TestSource::constant(MIXER_SAMPLE_RATE, 2, 32, 1.0),
        CueChainParams {
            fade_in_secs: 10.0 / MIXER_SAMPLE_RATE as f32,
            ..params()
        },
    );

    let rendered = harness.render_samples(&playback, 24);

    assert!(rendered[0].abs() < 0.01, "fade should start silent");
    assert!(
        rendered[10] > 0.4 && rendered[10] < 0.7,
        "fade midpoint was {}",
        rendered[10]
    );
    assert!(rendered[20] > 0.95, "fade should reach full volume");
}

#[test]
fn eq_option_forces_disabled_settings_on() {
    let mut harness = CueHarness::new(128);
    let playback = harness.play(
        TestSource::constant(MIXER_SAMPLE_RATE, 2, 4_096, 1.0),
        CueChainParams {
            eq: Some(EQSettings {
                enabled: false,
                hpf: EQFilter {
                    frequency: 100.0,
                    order: EQFilterOrder::_12dBOct,
                },
                ..Default::default()
            }),
            ..params()
        },
    );

    let rendered = harness.render_samples(&playback, 4_096);
    let settled_peak = rendered[rendered.len() - 128..]
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);

    assert!(
        settled_peak < 0.05,
        "forced-on HPF left DC peak {settled_peak}"
    );
}

#[test]
fn cue_chain_resamples_before_upmixing_to_device_format() {
    let source_frames = 2_048;
    let mut harness = CueHarness::new(128);
    let playback = harness.play(
        TestSource::constant(MIXER_SAMPLE_RATE / 2, 1, source_frames, 0.5),
        params(),
    );

    assert_eq!(playback.input.length(), Some(source_frames * 2 * 2));
    let rendered = harness.render_samples(&playback, 2_048);

    assert!(rendered.iter().any(|sample| sample.abs() > 0.1));
    for frame in rendered.chunks_exact(2) {
        assert!((frame[0] - frame[1]).abs() < 1e-6);
    }
}
