//! Peak file format — waveform cache for fast GUI rendering.
//!
//! Binary format (`.qpek`) with Brotli-compressed peak pyramids.
//! Matches C# `PeakFile` layout byte-for-byte.

use thiserror::Error;

/// Magic number: `QPeK` in little-endian.
pub const FILE_MAGIC: u32 = u32::from_le_bytes([b'Q', b'P', b'e', b'K']);
pub const FILE_VERSION: i32 = 5;
pub const FILE_EXTENSION: &str = ".qpek";

/// The reduction factor of the highest resolution pyramid.
pub const MIN_REDUCTION: i32 = 32;
/// Number of bits to shift the reduction factor between pyramid levels.
pub const REDUCTION_STEP: i32 = 1;
/// Minimum number of samples in a pyramid.
pub const MIN_SAMPLES: i32 = 64;
/// Sample position tracking increment (always a power of 2).
pub const SAMPLE_POS_INCREMENT: i32 = 1024;

/// A compact representation of audio peaks for fast waveform rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct PeakFile {
    // Metadata
    pub file_magic: u32,
    pub file_version: i32,
    pub source_file_length: i64,
    pub source_date: chrono::DateTime<chrono::Utc>,
    pub source_name_length: i32,
    pub source_name: String,

    // Peak data
    pub fs: i32,
    /// Total number of uncompressed mono samples in the source file.
    pub length: i64,
    pub peak_data_pyramid: Vec<PeakData>,
    pub peak: f32,

    // Seek helpers
    pub sample_pos_increment: i32,
    pub sample_pos_to_byte_pos: Vec<i64>,
}

impl Default for PeakFile {
    fn default() -> Self {
        Self {
            file_magic: FILE_MAGIC,
            file_version: FILE_VERSION,
            source_file_length: 0,
            source_date: chrono::DateTime::UNIX_EPOCH,
            source_name_length: 0,
            source_name: String::new(),
            fs: 48000,
            length: 0,
            peak_data_pyramid: Vec::new(),
            peak: 0.0,
            sample_pos_increment: SAMPLE_POS_INCREMENT,
            sample_pos_to_byte_pos: Vec::new(),
        }
    }
}

/// A union of peak + RMS values stored as u16 each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sample {
    pub peak: u16,
    pub rms: u16,
}

impl Sample {
    /// Pack into a single u32 (little-endian: peak | rms << 16).
    #[inline]
    pub fn to_u32(&self) -> u32 {
        (self.peak as u32) | ((self.rms as u32) << 16)
    }

    /// Unpack from u32.
    #[inline]
    pub fn from_u32(v: u32) -> Self {
        Self {
            peak: (v & 0xFFFF) as u16,
            rms: ((v >> 16) & 0xFFFF) as u16,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PeakData {
    /// How many source samples each entry at this level summarises.
    pub reduction_factor: i32,
    pub samples: Vec<Sample>,
}

#[derive(Debug, Error)]
pub enum PeakFileError {
    #[error("peak file is corrupt: {0}")]
    Malformed(String),
    #[error("peak file version {found} does not match expected {expected}")]
    InvalidVersion { found: i32, expected: i32 },
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Reads `.qpek` files from raw bytes.
pub struct PeakFileReader;

/// Writes `.qpek` files.
pub struct PeakFileWriter;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_magic() {
        assert_eq!(FILE_MAGIC, 0x4B655051); // 'Q' 'P' 'e' 'K' LE
    }

    #[test]
    fn test_sample_pack_unpack() {
        let s = Sample {
            peak: 0x1234,
            rms: 0x5678,
        };
        let packed = s.to_u32();
        let unpacked = Sample::from_u32(packed);
        assert_eq!(s, unpacked);
    }
}
