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
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Timestamp;

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
    decoder: Box<dyn AudioDecoder>,
    path: String,
    reopen_on_eof_seek: bool,
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
        if let Some(ext) = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
        {
            hint.with_extension(ext);
        }

        let mut format = symphonia::default::get_probe().probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;

        let reopen_on_eof_seek = format.format_info().short_name == "isomp4";
        let (track_id, mut sample_rate, mut channels, n_frames, codec_params) = {
            let track = format
                .first_track_known_codec(TrackType::Audio)
                .ok_or(DecodeError::NoAudioTrack)?;
            let codec_params = track
                .codec_params
                .as_ref()
                .and_then(|params| params.audio())
                .ok_or(DecodeError::NoAudioTrack)?;
            (
                track.id,
                codec_params.sample_rate,
                codec_params.channels.as_ref().map(|c| c.count() as u16),
                track.num_frames,
                codec_params.clone(),
            )
        };

        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&codec_params, &AudioDecoderOptions::default())?;

        // AAC in MP4 frequently omits the channel count (and occasionally the
        // sample rate) from the container — it's only known once the first packet
        // is decoded. Prime the decoder to learn the real spec instead of failing
        // with NoAudioTrack. Primed samples seed `residual` so none are lost.
        let mut residual: Vec<f32> = Vec::new();
        let mut eof = false;
        if sample_rate.is_none() || channels.is_none() {
            loop {
                match format.next_packet() {
                    Ok(Some(packet)) => {
                        if packet.track_id != track_id {
                            continue;
                        }
                        match decoder.decode(&packet) {
                            Ok(decoded) => {
                                sample_rate.get_or_insert(decoded.spec().rate());
                                channels.get_or_insert(decoded.spec().channels().count() as u16);
                                if decoded.frames() > 0 {
                                    decoded.copy_to_vec_interleaved(&mut residual);
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
                    Ok(None) | Err(_) => {
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
                path: path.to_owned(),
                reopen_on_eof_seek,
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
                Ok(Some(p)) => p,
                Ok(None) => {
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

            if packet.track_id != self.track_id {
                continue;
            }

            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    if decoded.frames() == 0 {
                        continue;
                    }
                    decoded.copy_to_vec_interleaved(&mut self.residual);
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
        let Ok(ts) = Timestamp::try_from(frame) else {
            log::warn!("seek target is out of range: {}", frame);
            return;
        };
        if inner.eof && inner.reopen_on_eof_seek {
            // ponytail: symphonia 0.6's ISO/MP4 reader cannot seek after Ok(None) (EOF clears the
            // pending mdat; seek does not restore it — pdeljanov/Symphonia#536). Reopen only at
            // EOF; remove this workaround when that fix lands. Commit the fresh reader only after
            // its seek succeeds — otherwise a failed seek (e.g. target past EOF) would swap in a
            // reader rewound to the start and replay audio instead of staying silent at EOF.
            let mut fresh = match Self::open(&inner.path) {
                Ok(f) => f.inner.into_inner(),
                Err(e) => {
                    log::warn!("seek reopen error: {}", e);
                    return;
                }
            };
            if let Err(e) = fresh.format.seek(
                SeekMode::Accurate,
                SeekTo::Timestamp {
                    ts,
                    track_id: fresh.track_id,
                },
            ) {
                log::warn!("seek error: {}", e);
                return;
            }
            *inner = fresh;
        } else if let Err(e) = inner.format.seek(
            SeekMode::Accurate,
            SeekTo::Timestamp {
                ts,
                track_id: inner.track_id,
            },
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
        assert!(
            max > 0.001 && max <= 1.0,
            "samples should be in [-1,1], got max {}",
            max
        );

        // A real signal has many zero crossings.
        let zc = buf[..read].windows(2).filter(|w| w[0] * w[1] < 0.0).count();
        assert!(zc > 100, "real audio should cross zero often, got {}", zc);
    }

    // Exercises the isomp4 reopen-on-EOF seek workaround (see seek()). Needs an
    // MP4 container, so build one from Ping.aiff with afconvert (ships with macOS).
    #[test]
    fn test_m4a_seek_after_eof() {
        if !std::path::Path::new(PING).exists() {
            return;
        }
        let m4a = std::env::temp_dir().join("cuepool_seek_eof_test.m4a");
        let converted = std::process::Command::new("afconvert")
            .args(["-f", "m4af", "-d", "aac", PING])
            .arg(&m4a)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !converted {
            return;
        }

        let decoder = FileDecoder::open(m4a.to_str().unwrap()).unwrap();
        let mut buf = vec![0.0f32; 4096];
        while decoder.read(&mut buf) > 0 {}

        // A seek past the end fails (isomp4 returns OutOfRange); the reader must
        // stay at EOF rather than being swapped for one rewound to the start.
        let len = decoder.length().unwrap_or(1 << 24);
        decoder.seek(len * 10);
        assert_eq!(
            decoder.read(&mut buf),
            0,
            "failed seek at EOF must stay silent"
        );

        // The workaround itself: a valid seek after EOF replays audio.
        decoder.seek(0);
        assert!(
            decoder.read(&mut buf) > 0,
            "valid seek after EOF should replay"
        );

        let _ = std::fs::remove_file(&m4a);
    }
}
