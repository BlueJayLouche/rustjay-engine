//! Linear Timecode (LTC) codec — biphase-mark encode/decode over f32 samples.
//!
//! Pure DSP with no audio-device dependencies: the encoder appends samples to
//! a caller-owned buffer, the decoder consumes mono f32 slices, so both are
//! unit-testable without hardware. Bit layout and the parity rule (bit 59 at
//! 25 fps, bit 27 otherwise, even parity over the whole 80-bit frame) follow
//! libltc's implementation of SMPTE ST 12-1; the decoder classifies
//! transition intervals (half-cell pairs are 1 bits, full cells are 0 bits)
//! with libltc's envelope-hysteresis front end and adaptive cell period.
//!
//! ponytail: reverse-playback decoding (reverse sync word 0xBFFE) is not
//! implemented — chase never shuttles backwards. Upgrade path: detect the
//! reverse sync word and re-read the buffered frame in reverse bit order,
//! as libltc's `parse_ltc` does.

use crate::midi::mtc::{MtcFrameRate, SmpteTime};

/// Bits per LTC frame (SMPTE ST 12-1).
pub const FRAME_BITS: usize = 80;

/// 16-bit sync word 0x3FFD, transmitted MSB-first in bits 64–79 so the
/// decoder's shift register equals this value when a frame completes.
const SYNC_WORD: u16 = 0x3FFD;

/// Exact fps for clock math — `MtcFrameRate::fps` rounds 30000/1001 to
/// 29.97, which would accumulate ~0.1 s/hour of sample-clock error.
fn exact_fps(rate: MtcFrameRate) -> f64 {
    match rate {
        MtcFrameRate::Fps2997Drop => 30000.0 / 1001.0,
        other => other.fps() as f64,
    }
}

// ── Frame bit layout ──────────────────────────────────────────────────────

fn set_bits(b: &mut [u8; FRAME_BITS], pos: usize, val: u8, n: usize) {
    for i in 0..n {
        b[pos + i] = (val >> i) & 1;
    }
}

fn get_bits(b: &[u8; 10], pos: usize, n: usize) -> u8 {
    let mut v = 0u8;
    for i in 0..n {
        v |= ((b[(pos + i) >> 3] >> ((pos + i) & 7)) & 1) << i;
    }
    v
}

/// Serialize a timecode to the 80-bit LTC frame (user/binary-group bits 0,
/// color-frame flag 0), including the biphase mark phase-correction bit.
fn frame_bits(tc: &SmpteTime) -> [u8; FRAME_BITS] {
    let mut b = [0u8; FRAME_BITS];
    set_bits(&mut b, 0, tc.frames % 10, 4);
    set_bits(&mut b, 8, tc.frames / 10, 2);
    b[10] = (tc.frame_rate == MtcFrameRate::Fps2997Drop) as u8;
    set_bits(&mut b, 16, tc.seconds % 10, 4);
    set_bits(&mut b, 24, tc.seconds / 10, 3);
    set_bits(&mut b, 32, tc.minutes % 10, 4);
    set_bits(&mut b, 40, tc.minutes / 10, 3);
    set_bits(&mut b, 48, tc.hours % 10, 4);
    set_bits(&mut b, 56, tc.hours / 10, 2);
    // Sync word: fixed bit pattern 0011 1111 1111 1101 across bits 64–79,
    // bit 64 first (i.e. the 0x3FFD value MSB-first).
    for i in 0..16 {
        b[64 + i] = ((SYNC_WORD >> (15 - i)) & 1) as u8;
    }
    // Even parity over all 80 bits, in bit 59 at 25 fps and bit 27 at other
    // rates, so every frame starts on the same biphase clock edge.
    let parity_bit = if tc.frame_rate == MtcFrameRate::Fps25 {
        59
    } else {
        27
    };
    let parity = b.iter().fold(0u8, |p, &bit| p ^ bit);
    b[parity_bit] = parity;
    b
}

// ── Encoder ───────────────────────────────────────────────────────────────

/// Encodes timecode frames as a biphase-mark f32 signal.
///
/// Carries fractional-sample timing across frames, so a continuous stream is
/// produced by repeated [`encode_frame`](LtcEncoder::encode_frame) calls.
pub struct LtcEncoder {
    /// Samples per bit cell (may be fractional, e.g. at 29.97 fps).
    samples_per_clock: f64,
    /// Fractional-sample carry between runs (libltc's `sample_remainder`).
    remainder: f64,
    /// Current output polarity.
    state: bool,
    /// Peak amplitude (0–1].
    amplitude: f64,
    /// One-pole lowpass coefficient shaping the edges (~40 µs rise time).
    filter_coeff: f64,
    filter_value: f64,
}

impl LtcEncoder {
    pub fn new(sample_rate: u32, frame_rate: MtcFrameRate) -> Self {
        Self {
            samples_per_clock: sample_rate as f64 / (exact_fps(frame_rate) * FRAME_BITS as f64),
            remainder: 0.5,
            state: false,
            amplitude: 0.5,
            // ST 12 wants a 40±10 µs rise time; one-pole lowpass, same
            // coefficient as libltc's `filter_const`.
            filter_coeff: 1.0
                - (-1.0 / (sample_rate as f64 * 0.000020 / std::f64::consts::E)).exp(),
            filter_value: 0.0,
        }
    }

    /// Output amplitude (peak, 0–1]. Default 0.5 ≈ −6 dBFS.
    pub fn set_amplitude(&mut self, amplitude: f32) {
        self.amplitude = amplitude.clamp(0.0, 1.0) as f64;
    }

    fn emit(&mut self, n: usize, out: &mut Vec<f32>) {
        let target = if self.state {
            self.amplitude
        } else {
            -self.amplitude
        };
        for _ in 0..n {
            self.filter_value += self.filter_coeff * (target - self.filter_value);
            out.push(self.filter_value as f32);
        }
    }

    fn encode_bit(&mut self, bit: u8, out: &mut Vec<f32>) {
        if bit == 0 {
            // Single transition at the cell boundary.
            let n = (self.samples_per_clock + self.remainder) as usize;
            self.remainder = self.samples_per_clock + self.remainder - n as f64;
            self.state = !self.state;
            self.emit(n, out);
        } else {
            // Transitions at the boundary and mid-cell.
            let half = self.samples_per_clock * 0.5;
            for _ in 0..2 {
                let n = (half + self.remainder) as usize;
                self.remainder = half + self.remainder - n as f64;
                self.state = !self.state;
                self.emit(n, out);
            }
        }
    }

    /// Append one frame of audio for `tc` to `out`. The frame rate comes from
    /// `tc.frame_rate`; it must match the rate passed to [`LtcEncoder::new`].
    pub fn encode_frame(&mut self, tc: &SmpteTime, out: &mut Vec<f32>) {
        for bit in frame_bits(tc) {
            self.encode_bit(bit, out);
        }
    }
}

// ── Decoder ───────────────────────────────────────────────────────────────

/// Decodes LTC from a mono f32 signal.
///
/// Polarity-invariant (it decodes transitions, not levels), tolerant of
/// amplitude variation and DC offset (thresholds track a decaying envelope),
/// and self-synchronizing: garbage or a mid-frame start costs at most one
/// frame before the sync word re-locks. The bit-cell period is adaptive, so
/// modest sample-rate mismatch (a decoder configured for 48 kHz fed a
/// 44.1 kHz stream) still decodes.
pub struct LtcDecoder {
    sample_rate: u32,
    /// Samples per bit cell, adapted on every transition (¼ smoothing).
    period: f64,
    /// Transition intervals above this count as a full cell (¾ of `period`).
    limit: f64,
    /// Samples since the last signal state change.
    count: u64,
    /// Quantized signal level (for hysteresis).
    state: bool,
    /// A half-cell interval waiting for its pair: transitions are always
    /// spaced by half a bit cell or a full one, and half-cell intervals come
    /// in pairs (boundary→mid, mid→boundary) — each pair is one 1 bit, each
    /// full-cell interval is one 0 bit.
    pending_short: bool,
    /// Signal envelope for the hysteresis thresholds.
    env_min: f64,
    env_max: f64,
    /// Frame bits in wire order, 8 per byte LSB-first.
    bits: [u8; FRAME_BITS / 8],
    bit_count: usize,
    sync_shift: u16,
}

impl LtcDecoder {
    pub fn new(sample_rate: u32) -> Self {
        // Seeded for 25 fps; the adaptive period converges within a few
        // transitions if the source is 24 or 30 fps.
        let period = sample_rate as f64 / (25.0 * FRAME_BITS as f64);
        Self {
            sample_rate,
            period,
            limit: period * 0.75,
            // Starts "after silence": the first detected transition only
            // re-arms the machine, it emits nothing.
            count: u64::MAX / 2,
            state: false,
            pending_short: false,
            env_min: 0.0,
            env_max: 0.0,
            bits: [0; FRAME_BITS / 8],
            bit_count: 0,
            sync_shift: 0,
        }
    }

    /// Feed mono samples; returns any frames that completed.
    pub fn feed(&mut self, samples: &[f32]) -> Vec<SmpteTime> {
        let mut out = Vec::new();
        for &sample in samples {
            let s = sample as f64;
            // Envelope: decay toward 0, then track the sample.
            self.env_min *= 15.0 / 16.0;
            self.env_max *= 15.0 / 16.0;
            self.env_min = self.env_min.min(s);
            self.env_max = self.env_max.max(s);
            let min_threshold = self.env_min * 0.5;
            let max_threshold = self.env_max * 0.5;

            if (self.state && s > max_threshold) || (!self.state && s < min_threshold) {
                self.on_transition(&mut out);
                self.count = 0;
                self.state = !self.state;
            }
            self.count += 1;
        }
        out
    }

    /// Classify the interval since the previous transition and emit bits.
    /// Intervals are half a bit cell (a mid-cell transition of a 1 bit) or a
    /// full cell (a 0 bit); the ¾-cell limit separates them, and the measured
    /// interval adapts the cell period for speed/sample-rate drift.
    fn on_transition(&mut self, out: &mut Vec<SmpteTime>) {
        let cnt = self.count as f64;
        if cnt > self.period * 4.0 {
            // Long silence — drop any partial frame and pairing; the sync
            // word re-locks framing once the signal returns.
            self.bit_count = 0;
            self.pending_short = false;
            return;
        }
        if cnt > self.limit {
            // Full cell: a 0 bit. An unpaired half-cell before it means the
            // pairing slipped (garbage/startup) — force re-framing.
            if self.pending_short {
                self.pending_short = false;
                self.bit_count = 0;
            }
            self.push_bit(0, out);
            self.period = (self.period * 3.0 + cnt) / 4.0;
        } else {
            if self.pending_short {
                self.push_bit(1, out);
                self.pending_short = false;
            } else {
                self.pending_short = true;
            }
            self.period = (self.period * 3.0 + cnt * 2.0) / 4.0;
        }
        self.limit = self.period * 0.75;
    }

    fn push_bit(&mut self, bit: u8, out: &mut Vec<SmpteTime>) {
        if self.bit_count == 0 {
            self.bits = [0; FRAME_BITS / 8];
        }
        if self.bit_count >= FRAME_BITS {
            // Run-on (spurious extra bit since the last sync): shift the
            // frame back one bit so the next sync word can re-align it.
            for k in 0..FRAME_BITS / 8 {
                let mut shifted = self.bits[k] >> 1;
                if k + 1 < FRAME_BITS / 8 {
                    shifted |= (self.bits[k + 1] & 1) << 7;
                }
                self.bits[k] = shifted;
            }
            self.bit_count -= 1;
        }
        self.sync_shift = (self.sync_shift << 1) | (bit as u16);
        if bit != 0 && self.bit_count < FRAME_BITS {
            self.bits[self.bit_count >> 3] |= 1 << (self.bit_count & 7);
        }
        self.bit_count += 1;
        if self.sync_shift == SYNC_WORD {
            if self.bit_count == FRAME_BITS
                && let Some(tc) = self.assemble()
            {
                out.push(tc);
            }
            self.bit_count = 0;
        }
    }

    fn assemble(&self) -> Option<SmpteTime> {
        let b = &self.bits;
        let frames = get_bits(b, 0, 4) + 10 * get_bits(b, 8, 2);
        let seconds = get_bits(b, 16, 4) + 10 * get_bits(b, 24, 3);
        let minutes = get_bits(b, 32, 4) + 10 * get_bits(b, 40, 3);
        let hours = get_bits(b, 48, 4) + 10 * get_bits(b, 56, 2);
        // Reject garbage that happened to land on the sync word.
        if frames >= 30 || seconds >= 60 || minutes >= 60 || hours >= 24 {
            return None;
        }
        let drop_frame = get_bits(b, 10, 1) != 0;
        // The frame rate has no wire encoding — derive it from the measured
        // bit-cell period. Timing cannot separate 29.97 from 30 (1.6 samples
        // per frame at 48 kHz), so the drop-frame flag decides between them.
        let fps = self.sample_rate as f64 / (self.period * FRAME_BITS as f64);
        let frame_rate = if fps < 24.5 {
            MtcFrameRate::Fps24
        } else if fps < 27.5 {
            MtcFrameRate::Fps25
        } else if drop_frame {
            MtcFrameRate::Fps2997Drop
        } else {
            MtcFrameRate::Fps30
        };
        Some(SmpteTime {
            hours,
            minutes,
            seconds,
            frames,
            frame_rate,
        })
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(hours: u8, minutes: u8, seconds: u8, frames: u8, frame_rate: MtcFrameRate) -> SmpteTime {
        SmpteTime {
            hours,
            minutes,
            seconds,
            frames,
            frame_rate,
        }
    }

    /// Advance a timecode by one frame (naive: no drop-frame label skipping —
    /// the codec round-trips labels, it does not number them).
    fn increment(tc: &mut SmpteTime) {
        let nominal = match tc.frame_rate {
            MtcFrameRate::Fps2997Drop => 30,
            other => other.fps() as u8,
        };
        tc.frames += 1;
        if tc.frames >= nominal {
            tc.frames = 0;
            tc.seconds += 1;
            if tc.seconds >= 60 {
                tc.seconds = 0;
                tc.minutes += 1;
                if tc.minutes >= 60 {
                    tc.minutes = 0;
                    tc.hours += 1;
                }
            }
        }
    }

    /// Encode `n` consecutive frames from `start`, then transform the samples.
    fn signal(start: SmpteTime, n: usize, sample_rate: u32, f: impl FnMut(f32) -> f32) -> Vec<f32> {
        let mut enc = LtcEncoder::new(sample_rate, start.frame_rate);
        let mut tc = start;
        let mut buf = Vec::new();
        for _ in 0..n {
            enc.encode_frame(&tc, &mut buf);
            increment(&mut tc);
        }
        buf.iter().copied().map(f).collect()
    }

    /// Decode the whole buffer in 511-sample chunks (odd size on purpose —
    /// capture delivers arbitrary chunk boundaries).
    fn decode_chunked(dec: &mut LtcDecoder, samples: &[f32]) -> Vec<SmpteTime> {
        let mut out = Vec::new();
        for chunk in samples.chunks(511) {
            out.extend(dec.feed(chunk));
        }
        out
    }

    fn assert_roundtrip(start: SmpteTime, n: usize, decoded: &[SmpteTime]) {
        let mut expected = Vec::new();
        let mut t = start;
        for _ in 0..n {
            expected.push(t);
            increment(&mut t);
        }
        // The first transition after startup/silence is consumed by re-sync
        // and the final frame's last bit needs the next frame's first
        // transition, so up to one frame may be missing at each end.
        assert!(!decoded.is_empty(), "nothing decoded");
        let offset = expected
            .iter()
            .position(|t| Some(t) == decoded.first())
            .expect("first decoded frame not in the expected sequence");
        assert!(offset <= 1, "skipped {offset} frames at startup");
        assert!(
            offset + decoded.len() + 1 >= n,
            "decoded {} frames from {n} (offset {offset})",
            decoded.len()
        );
        assert_eq!(decoded, &expected[offset..offset + decoded.len()]);
    }

    #[test]
    fn roundtrip_all_frame_rates() {
        for &rate in &[
            MtcFrameRate::Fps24,
            MtcFrameRate::Fps25,
            MtcFrameRate::Fps2997Drop,
            MtcFrameRate::Fps30,
        ] {
            let start = tc(1, 0, 0, 0, rate);
            let samples = signal(start, 100, 48000, |s| s);
            let mut dec = LtcDecoder::new(48000);
            let decoded = decode_chunked(&mut dec, &samples);
            assert_roundtrip(start, 100, &decoded);
        }
    }

    #[test]
    fn frame_layout_and_parity() {
        // 01:02:03:04 @ 25fps — spot-check BCD fields, sync word, drop flag.
        let b = frame_bits(&tc(1, 2, 3, 4, MtcFrameRate::Fps25));
        assert_eq!(&b[0..4], &[0, 0, 1, 0]); // frames units = 4
        assert_eq!(&b[8..10], &[0, 0]); // frames tens = 0
        assert_eq!(b[10], 0); // not drop-frame
        assert_eq!(&b[16..20], &[1, 1, 0, 0]); // seconds units = 3
        assert_eq!(&b[32..36], &[0, 1, 0, 0]); // minutes units = 2
        assert_eq!(&b[48..52], &[1, 0, 0, 0]); // hours units = 1
        let mut sync = 0u16;
        for i in 0..16 {
            sync |= (b[64 + i] as u16) << (15 - i);
        }
        assert_eq!(sync, SYNC_WORD);
        // Even parity over all 80 bits at every rate.
        for &rate in &[
            MtcFrameRate::Fps24,
            MtcFrameRate::Fps25,
            MtcFrameRate::Fps2997Drop,
            MtcFrameRate::Fps30,
        ] {
            let b = frame_bits(&tc(23, 59, 58, 17, rate));
            assert_eq!(b.iter().fold(0u8, |p, &bit| p ^ bit), 0, "rate {rate:?}");
        }
        assert_eq!(
            frame_bits(&tc(0, 0, 0, 0, MtcFrameRate::Fps2997Drop))[10],
            1
        );
    }

    #[test]
    fn decodes_at_low_and_varying_amplitude() {
        let start = tc(2, 30, 15, 7, MtcFrameRate::Fps25);
        // Constant −20 dB.
        let quiet = signal(start, 60, 48000, |s| s * 0.1);
        let mut dec = LtcDecoder::new(48000);
        assert_roundtrip(start, 60, &decode_chunked(&mut dec, &quiet));
        // Slow ramp from near silence to full level — the envelope tracker
        // must follow without losing a frame.
        let ramp = {
            let raw = signal(start, 60, 48000, |s| s);
            let len = raw.len() as f32;
            raw.iter()
                .enumerate()
                .map(|(i, &s)| s * (0.05 + 0.95 * i as f32 / len))
                .collect::<Vec<_>>()
        };
        let mut dec = LtcDecoder::new(48000);
        assert_roundtrip(start, 60, &decode_chunked(&mut dec, &ramp));
    }

    #[test]
    fn decodes_polarity_inverted() {
        let start = tc(0, 5, 0, 3, MtcFrameRate::Fps30);
        let samples = signal(start, 60, 48000, |s| -s);
        let mut dec = LtcDecoder::new(48000);
        assert_roundtrip(start, 60, &decode_chunked(&mut dec, &samples));
    }

    #[test]
    fn decodes_with_sample_rate_mismatch() {
        // Stream actually at 44.1 kHz, decoder told 48 kHz: the adaptive
        // bit-cell period must absorb the 9% error.
        let start = tc(1, 0, 30, 0, MtcFrameRate::Fps25);
        let samples = signal(start, 60, 44100, |s| s);
        let mut dec = LtcDecoder::new(48000);
        assert_roundtrip(start, 60, &decode_chunked(&mut dec, &samples));
    }

    #[test]
    fn resyncs_after_mid_frame_start() {
        let start = tc(3, 0, 0, 0, MtcFrameRate::Fps25);
        let samples = signal(start, 50, 48000, |s| s);
        // Cut into the middle of the first frame.
        let mut dec = LtcDecoder::new(48000);
        let decoded = decode_chunked(&mut dec, &samples[317..]);
        // The partial first frame is lost; everything after the first sync
        // word must decode correctly.
        let mut expected = start;
        increment(&mut expected);
        assert!(
            !decoded.is_empty() && decoded.len() >= 48,
            "expected ≥48 frames, got {}",
            decoded.len()
        );
        assert_eq!(decoded[0], expected);
        assert_roundtrip(expected, decoded.len(), &decoded);
    }

    #[test]
    fn decodes_through_noise() {
        let start = tc(0, 0, 10, 0, MtcFrameRate::Fps25);
        // Deterministic pseudo-noise (LCG) at 20% of the signal amplitude.
        let mut rng = 0x12345678u32;
        let samples = signal(start, 60, 48000, |s| {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = (rng >> 24) as f32 / 255.0 - 0.5;
            s + noise * 0.1
        });
        let mut dec = LtcDecoder::new(48000);
        assert_roundtrip(start, 60, &decode_chunked(&mut dec, &samples));
    }

    #[test]
    fn silence_yields_nothing_and_recovers() {
        let start = tc(0, 0, 0, 5, MtcFrameRate::Fps25);
        let mut dec = LtcDecoder::new(48000);
        assert!(dec.feed(&[0.0; 4800]).is_empty());
        let samples = signal(start, 30, 48000, |s| s);
        assert_roundtrip(start, 30, &decode_chunked(&mut dec, &samples));
    }
}
