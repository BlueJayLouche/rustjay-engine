//! # Texture Utilities
//!
//! Cross-platform texture helpers for row pitch alignment and format conversions.
//!
//! ## Row Pitch Alignment
//!
//! wgpu requires 256-byte aligned row pitch for COPY operations:
//! - `copy_texture_to_buffer`
//! - `copy_buffer_to_texture`
//! - `write_texture`
//!
/// Align a value to the next multiple of alignment.
///
/// # Examples
/// ```
/// assert_eq!(align_to(100, 256), 256);
/// assert_eq!(align_to(256, 256), 256);
/// assert_eq!(align_to(257, 256), 512);
/// ```
pub const fn align_to(value: u32, alignment: u32) -> u32 {
    ((value + alignment - 1) / alignment) * alignment
}

/// Calculate 256-byte aligned row pitch for BGRA8 textures.
///
/// wgpu requires COPY operations to use 256-byte aligned row pitches.
/// This ensures compatibility across all backends (D3D12, Vulkan, Metal).
///
/// # Arguments
/// * `width` - Texture width in pixels
///
/// # Returns
/// Row pitch in bytes, aligned to 256 bytes
///
/// # Examples
/// ```
/// // 1920x1080 BGRA: 7680 bytes per row, already aligned
/// assert_eq!(aligned_row_pitch_bgra(1920), 7680);
///
/// // 1000px width: 4000 bytes, aligned to 4096
/// assert_eq!(aligned_row_pitch_bgra(1000), 4096);
/// ```
pub fn aligned_row_pitch_bgra(width: u32) -> u32 {
    const BYTES_PER_PIXEL: u32 = 4;
    const ALIGNMENT: u32 = 256;
    align_to(width * BYTES_PER_PIXEL, ALIGNMENT)
}

/// Calculate row pitch without alignment (for display calculations).
pub fn unaligned_row_pitch_bgra(width: u32) -> u32 {
    width * 4
}

/// Get the padding bytes per row for an aligned buffer.
pub fn row_padding_bytes(width: u32) -> u32 {
    aligned_row_pitch_bgra(width) - unaligned_row_pitch_bgra(width)
}

/// DXGI format constants for Spout2 protocol compatibility.
pub mod dxgi_format {
    /// B8G8R8A8_UNORM - Standard BGRA 8-bit per channel
    /// Value: 87
    pub const B8G8R8A8_UNORM: u32 = 87;

    /// R8G8B8A8_UNORM - Standard RGBA 8-bit per channel
    /// Value: 28
    pub const R8G8B8A8_UNORM: u32 = 28;

    /// B8G8R8A8_UNORM_SRGB - BGRA with sRGB gamma
    /// Value: 91
    pub const B8G8R8A8_UNORM_SRGB: u32 = 91;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_align_to() {
        assert_eq!(align_to(0, 256), 0);
        assert_eq!(align_to(1, 256), 256);
        assert_eq!(align_to(255, 256), 256);
        assert_eq!(align_to(256, 256), 256);
        assert_eq!(align_to(257, 256), 512);
        assert_eq!(align_to(512, 256), 512);
        assert_eq!(align_to(7680, 256), 7680); // 1920*4
    }

    #[test]
    fn test_aligned_row_pitch_bgra() {
        // Common resolutions
        assert_eq!(aligned_row_pitch_bgra(1920), 7680); // 1080p - already aligned
        assert_eq!(aligned_row_pitch_bgra(1280), 5120); // 720p - already aligned
        assert_eq!(aligned_row_pitch_bgra(3840), 15360); // 4K - already aligned

        // Non-aligned cases
        assert_eq!(aligned_row_pitch_bgra(100), 512); // 400 -> 512
        assert_eq!(aligned_row_pitch_bgra(1000), 4096); // 4000 -> 4096
    }

    #[test]
    fn test_dxgi_constants() {
        assert_eq!(dxgi_format::B8G8R8A8_UNORM, 87);
        assert_eq!(dxgi_format::R8G8B8A8_UNORM, 28);
    }
}
