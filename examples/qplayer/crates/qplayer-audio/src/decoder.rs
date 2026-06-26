//! Pure-Rust audio file decoder (symphonia).
//!
//! Opens WAV, MP3, FLAC, OGG/Vorbis, AIFF, AAC/M4A and converts to
//! interleaved f32. Replaces the previous FFmpeg-based decoder.
//!
//! `read()` is called from the buffered-source background thread (never the
//! audio callback), so allocating during decode is acceptable.

use crate::SampleProvider;
use std::cell::UnsafeCell;
use std::fs::File;
use std::sync::atomic::{AtomicUsize, Ordering};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("no decodable audio track in file")]
    NoAudioTrack,
    #[error(transparent)]
    Symphonia(#[from] SymError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Pure-Rust audio file decoder.
pub struct FileDecoder {
    inner: UnsafeCell<Inner>,
    // Immutable metadata + position live outside the UnsafeCell so the read-only
    // trait methods never alias the `&mut Inner` that `read`/`seek` take.
    sample_rate: u32,
    channels: u16,
    total_samples: Option<usize>,
    position: AtomicUsize,
}

struct Inner {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    /// Converted-but-unconsumed interleaved f32 samples.
    residual: Vec<f32>,
    residual_pos: usize,
    eof: bool,
}

impl FileDecoder {
    pub fn open(path: &str) -> Result<Self, DecodeError> {
        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = std::path::Path::new(path).extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let probed = symphonia::default::get_probe().format(
            &hint,
            mss,
            &FormatOptions { enable_gapless: true, ..Default::default() },
            &MetadataOptions::default(),
        )?;
        let mut format = probed.format;

        let (track_id, mut sample_rate, mut channels, n_frames, codec_params) = {
            let track = format
                .tracks()
                .iter()
                .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
                .ok_or(DecodeError::NoAudioTrack)?;
            (
                track.id,
                track.codec_params.sample_rate,
                track.codec_params.channels.map(|c| c.count() as u16),
                track.codec_params.n_frames,
                track.codec_params.clone(),
            )
        };

        let mut decoder = symphonia::default::get_codecs()
            .make(&codec_params, &DecoderOptions::default())?;

        // AAC in MP4 frequently omits the channel count (and occasionally the
        // sample rate) from the container — it's only known once the first packet
        // is decoded. Prime the decoder to learn the real spec instead of failing
        // with NoAudioTrack. Primed samples seed `residual` so none are lost.
        let mut residual: Vec<f32> = Vec::new();
        let mut eof = false;
        if sample_rate.is_none() || channels.is_none() {
            loop {
                match format.next_packet() {
                    Ok(packet) => {
                        if packet.track_id() != track_id {
                            continue;
                        }
                        match decoder.decode(&packet) {
                            Ok(decoded) => {
                                let spec = *decoded.spec();
                                sample_rate.get_or_insert(spec.rate);
                                channels.get_or_insert(spec.channels.count() as u16);
                                if decoded.frames() > 0 {
                                    let mut sbuf =
                                        SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                                    sbuf.copy_interleaved_ref(decoded);
                                    residual.extend_from_slice(sbuf.samples());
                                }
                                break;
                            }
                            Err(SymError::DecodeError(_)) => continue,
                            Err(_) => {
                                eof = true;
                                break;
                            }
                        }
                    }
                    Err(_) => {
                        eof = true;
                        break;
                    }
                }
            }
        }

        let sample_rate = sample_rate.ok_or(DecodeError::NoAudioTrack)?;
        let channels = channels.ok_or(DecodeError::NoAudioTrack)?;
        let total_samples = n_frames.map(|f| f as usize * channels as usize);

        Ok(Self {
            inner: UnsafeCell::new(Inner {
                format,
                decoder,
                track_id,
                residual,
                residual_pos: 0,
                eof,
            }),
            sample_rate,
            channels,
            total_samples,
            position: AtomicUsize::new(0),
        })
    }
}

impl Inner {
    /// Decode the next audio packet into `residual`. Returns false at EOF.
    fn fill_residual(&mut self) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.eof = true;
                    return false;
                }
                // ponytail: chained OGG streams (ResetRequired) treated as EOF — v1 doesn't
                // need gapless stream-chaining. Re-make the decoder here if it ever matters.
                Err(SymError::ResetRequired) => {
                    self.eof = true;
                    return false;
                }
                Err(e) => {
                    log::warn!("next_packet error: {}", e);
                    self.eof = true;
                    return false;
                }
            };

            if packet.track_id() != self.track_id {
                continue;
            }

            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    if decoded.frames() == 0 {
                        continue;
                    }
                    // ponytail: one SampleBuffer alloc per packet — BG decode thread, not the
                    // audio callback. Reuse a max-sized buffer if profiling ever flags it.
                    let mut sbuf = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
                    sbuf.copy_interleaved_ref(decoded);
                    self.residual.clear();
                    self.residual.extend_from_slice(sbuf.samples());
                    self.residual_pos = 0;
                    return true;
                }
                Err(SymError::DecodeError(e)) => {
                    log::warn!("decode error (skipping packet): {}", e);
                    continue;
                }
                Err(e) => {
                    log::warn!("fatal decode error: {}", e);
                    self.eof = true;
                    return false;
                }
            }
        }
    }

    fn read_into(&mut self, buffer: &mut [f32]) -> usize {
        let mut written = 0;
        while written < buffer.len() {
            if self.residual_pos < self.residual.len() {
                let avail = self.residual.len() - self.residual_pos;
                let n = avail.min(buffer.len() - written);
                buffer[written..written + n]
                    .copy_from_slice(&self.residual[self.residual_pos..self.residual_pos + n]);
                self.residual_pos += n;
                written += n;
                continue;
            }
            if self.eof || !self.fill_residual() {
                break;
            }
        }
        written
    }
}

impl SampleProvider for FileDecoder {
    fn read(&self, buffer: &mut [f32]) -> usize {
        let inner = unsafe { &mut *self.inner.get() };
        let n = inner.read_into(buffer);
        self.position.fetch_add(n, Ordering::Relaxed);
        n
    }

    fn seek(&self, sample: usize) {
        let inner = unsafe { &mut *self.inner.get() };
        let frame = (sample / self.channels.max(1) as usize) as u64;
        if let Err(e) = inner.format.seek(
            SeekMode::Accurate,
            SeekTo::TimeStamp { ts: frame, track_id: inner.track_id },
        ) {
            log::warn!("seek error: {}", e);
            return;
        }
        inner.decoder.reset();
        inner.residual.clear();
        inner.residual_pos = 0;
        inner.eof = false;
        self.position.store(sample, Ordering::Relaxed);
    }

    fn position(&self) -> usize {
        self.position.load(Ordering::Relaxed)
    }

    fn length(&self) -> Option<usize> {
        self.total_samples
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> u16 {
        self.channels
    }
}

// read/seek are serialized by the BufferedSource mutex; metadata reads are atomic/immutable.
unsafe impl Send for FileDecoder {}
unsafe impl Sync for FileDecoder {}

#[cfg(test)]
mod tests {
    use super::*;

    // macOS ships this file; skip gracefully elsewhere (CI/Linux).
    const PING: &str = "/System/Library/Sounds/Ping.aiff";

    #[test]
    fn test_open_ping() {
        if !std::path::Path::new(PING).exists() {
            return;
        }
        let decoder = FileDecoder::open(PING).unwrap();
        assert!(decoder.sample_rate() > 0);
        assert!(decoder.channels() >= 1);
        assert!(decoder.length().unwrap() > 0);
    }

    #[test]
    fn test_decode_ping_in_range() {
        if !std::path::Path::new(PING).exists() {
            return;
        }
        let decoder = FileDecoder::open(PING).unwrap();
        let mut buf = vec![0.0f32; decoder.sample_rate() as usize * decoder.channels() as usize];
        let read = decoder.read(&mut buf);
        assert!(read > 0, "should decode some samples");

        let max = buf[..read].iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max > 0.001 && max <= 1.0, "samples should be in [-1,1], got max {}", max);

        // A real signal has many zero crossings.
        let zc = buf[..read].windows(2).filter(|w| w[0] * w[1] < 0.0).count();
        assert!(zc > 100, "real audio should cross zero often, got {}", zc);
    }
}
