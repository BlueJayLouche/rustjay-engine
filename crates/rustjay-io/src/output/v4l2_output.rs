//! # V4L2 Loopback Output (Linux)
//!
//! Writes frames to a `/dev/videoN` loopback device created by the `v4l2loopback`
//! kernel module.  Other applications (OBS, ffplay, browsers) can read from the
//! virtual camera as if it were a real webcam.
//!
//! ## Setup
//!
//! Load the kernel module and create a virtual device:
//! ```bash
//! sudo modprobe v4l2loopback devices=1 video_nr=10 \
//!     card_label="RustJay Output" exclusive_caps=1
//! ```
//!
//! ## Format
//!
//! We negotiate **YUYV** (`YUY2`) with the device — it is the format most
//! consumer apps (browsers, OBS, ffplay) expect from a webcam.  Incoming
//! BGRA frames are converted to YUYV in a pre-allocated scratch buffer
//! before being written.

#![cfg(target_os = "linux")]

use std::io::Write;

use v4l::video::Output;
use v4l::{Device, Format, FourCC};

/// Writes BGRA frames to a V4L2 loopback device.
pub struct V4l2LoopbackOutput {
    device_path: String,
    device: Device,
    width: u32,
    height: u32,
    /// Reused YUYV scratch buffer (`width * height * 2` bytes).
    yuyv_scratch: Vec<u8>,
    /// Have we already logged a transient write error?  Throttles logs so a
    /// disconnected consumer doesn't spam every frame.
    warned_transient: bool,
}

impl V4l2LoopbackOutput {
    /// Open the loopback device and negotiate YUYV at the requested size.
    pub fn new(device_path: &str, width: u32, height: u32) -> anyhow::Result<Self> {
        let device = open_and_configure(device_path, width, height)?;
        let yuyv_scratch = vec![0u8; (width as usize) * (height as usize) * 2];

        log::info!(
            "V4L2 output opened: {} ({}x{} YUYV)",
            device_path,
            width,
            height
        );

        Ok(Self {
            device_path: device_path.to_string(),
            device,
            width,
            height,
            yuyv_scratch,
            warned_transient: false,
        })
    }

    /// Write one BGRA frame to the loopback device.
    ///
    /// If `width`/`height` differs from the currently-configured size, the
    /// device is reopened and renegotiated at the new size.
    pub fn send_frame(&mut self, bgra: &[u8], width: u32, height: u32) -> anyhow::Result<()> {
        if width != self.width || height != self.height {
            log::info!(
                "V4L2 resolution change: {}x{} -> {}x{} (reopening {})",
                self.width,
                self.height,
                width,
                height,
                self.device_path
            );
            self.device = open_and_configure(&self.device_path, width, height)?;
            self.width = width;
            self.height = height;
            self.yuyv_scratch
                .resize((width as usize) * (height as usize) * 2, 0);
            self.warned_transient = false;
        }

        let expected = (width as usize) * (height as usize) * 4;
        if bgra.len() < expected {
            anyhow::bail!(
                "V4L2 frame too small: got {} bytes, expected {}",
                bgra.len(),
                expected
            );
        }

        bgra_to_yuyv(&bgra[..expected], &mut self.yuyv_scratch, width, height);

        match self.device.write_all(&self.yuyv_scratch) {
            Ok(()) => {
                self.warned_transient = false;
                Ok(())
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                if !self.warned_transient {
                    log::warn!(
                        "V4L2 transient write error on {} ({}); continuing",
                        self.device_path,
                        e
                    );
                    self.warned_transient = true;
                }
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!(
                "V4L2 write error on {}: {}",
                self.device_path,
                e
            )),
        }
    }

    /// Current output resolution.
    pub fn resolution(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// Open the device and call `VIDIOC_S_FMT` to pin it to `width x height` YUYV.
fn open_and_configure(path: &str, width: u32, height: u32) -> anyhow::Result<Device> {
    let device = Device::with_path(path)
        .map_err(|e| anyhow::anyhow!("Failed to open V4L2 device {}: {}", path, e))?;

    if let Ok(caps) = device.query_caps() {
        log::info!(
            "V4L2 device {}: driver={} card={} caps={:?}",
            path,
            caps.driver,
            caps.card,
            caps.capabilities
        );
    }

    let requested = Format::new(width, height, FourCC::new(b"YUYV"));
    let applied = <Device as Output>::set_format(&device, &requested).map_err(|e| {
        anyhow::anyhow!(
            "V4L2 set_format({}x{} YUYV) on {} failed: {} — is v4l2loopback loaded?",
            width,
            height,
            path,
            e
        )
    })?;

    if applied.fourcc != FourCC::new(b"YUYV") {
        anyhow::bail!(
            "V4L2 device {} negotiated unexpected format {} instead of YUYV",
            path,
            applied.fourcc
        );
    }

    if applied.width != width || applied.height != height {
        anyhow::bail!(
            "V4L2 device {} refused {}x{} (returned {}x{})",
            path,
            width,
            height,
            applied.width,
            applied.height
        );
    }

    Ok(device)
}

/// Convert a BGRA frame to packed YUYV (Y0 U Y1 V), BT.601 limited-range.
///
/// Assumes `bgra.len() >= width*height*4` and `yuyv.len() == width*height*2`.
/// Chroma is averaged between each horizontally-adjacent pixel pair.
fn bgra_to_yuyv(bgra: &[u8], yuyv: &mut [u8], width: u32, height: u32) {
    let w = width as usize;
    let h = height as usize;
    let pairs = w / 2;
    let leftover = w % 2;

    for y in 0..h {
        let row_bgra = &bgra[y * w * 4..(y + 1) * w * 4];
        let row_yuyv = &mut yuyv[y * w * 2..(y + 1) * w * 2];

        for x in 0..pairs {
            let p0 = &row_bgra[x * 8..x * 8 + 4];
            let p1 = &row_bgra[x * 8 + 4..x * 8 + 8];

            let (y0, u0, v0) = rgb_to_yuv(p0[2], p0[1], p0[0]);
            let (y1, u1, v1) = rgb_to_yuv(p1[2], p1[1], p1[0]);

            let u = ((u0 as u16 + u1 as u16) / 2) as u8;
            let v = ((v0 as u16 + v1 as u16) / 2) as u8;

            row_yuyv[x * 4] = y0;
            row_yuyv[x * 4 + 1] = u;
            row_yuyv[x * 4 + 2] = y1;
            row_yuyv[x * 4 + 3] = v;
        }

        if leftover == 1 {
            let off = pairs * 8;
            let p = &row_bgra[off..off + 4];
            let (y0, u, v) = rgb_to_yuv(p[2], p[1], p[0]);
            let yoff = pairs * 4;
            row_yuyv[yoff] = y0;
            row_yuyv[yoff + 1] = u;
            row_yuyv[yoff + 2] = y0;
            row_yuyv[yoff + 3] = v;
        }
    }
}

/// BT.601 limited-range RGB → YUV (Y in [16,235], Cb/Cr in [16,240]).
#[inline]
fn rgb_to_yuv(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let r = r as i32;
    let g = g as i32;
    let b = b as i32;

    let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
    let cb = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let cr = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;

    (
        y.clamp(0, 255) as u8,
        cb.clamp(0, 255) as u8,
        cr.clamp(0, 255) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_bgra_maps_to_black_yuyv() {
        let bgra = vec![0u8; 4 * 4]; // 2x2 BGRA, all zero
        let mut yuyv = vec![0u8; 4 * 2];
        bgra_to_yuyv(&bgra, &mut yuyv, 2, 2);
        // Y ~16, U/V ~128 for black in limited-range BT.601
        for row in 0..2 {
            let base = row * 4;
            assert_eq!(yuyv[base], 16);
            assert_eq!(yuyv[base + 2], 16);
            assert_eq!(yuyv[base + 1], 128);
            assert_eq!(yuyv[base + 3], 128);
        }
    }

    #[test]
    fn white_bgra_maps_to_near_white_luma() {
        // One row, two pixels, pure white (BGRA = 255,255,255,255)
        let bgra = vec![255u8; 2 * 4];
        let mut yuyv = vec![0u8; 2 * 2];
        bgra_to_yuyv(&bgra, &mut yuyv, 2, 1);
        // Y ~235 for white in limited-range BT.601
        assert!(yuyv[0] >= 234 && yuyv[0] <= 236);
        assert!(yuyv[2] >= 234 && yuyv[2] <= 236);
    }
}
