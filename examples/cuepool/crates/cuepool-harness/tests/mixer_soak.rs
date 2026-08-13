use cuepool_audio::{Mixer, MixerInput, MIXER_CHANNELS, MIXER_SAMPLE_RATE};
use cuepool_harness::sink::{NullSink, RampSource};
use std::sync::Arc;

/// Renders one simulated hour of audio through the mixer with sources being
/// added and removed, asserting the output stays finite and in range.
/// A NaN or an infinity here is a silent killer on the real rig: it propagates
/// into the driver and the room goes to full-scale noise.
#[test]
fn mixer_survives_one_simulated_hour() {
    let mixer = Arc::new(Mixer::new(MIXER_CHANNELS, MIXER_SAMPLE_RATE));
    let block_frames = 512;
    let mut sink = NullSink::new(Arc::clone(&mixer), block_frames);

    let blocks_per_hour = (MIXER_SAMPLE_RATE as usize * 3600) / block_frames;
    let mut rng = cuepool_harness::rng::Xorshift64::new(0xC0FFEE);

    for block in 0..blocks_per_hour {
        if block % 97 == 0 {
            let len = rng.next_range(1_000, 200_000) as usize;
            let src = RampSource::new(MIXER_SAMPLE_RATE, MIXER_CHANNELS, len);
            let input = Arc::new(MixerInput::new(Box::new(src), block_frames * 4));
            input.set_volume(rng.next_range(0, 100) as f32 / 100.0);
            input.set_active(true);
            mixer.add_input(input);
        }
        let out = sink.render_block();
        for (i, s) in out.iter().enumerate() {
            assert!(s.is_finite(), "non-finite sample at block {block} idx {i}: {s}");
            assert!(s.abs() <= 8.0, "runaway sample at block {block} idx {i}: {s}");
        }
    }
}
