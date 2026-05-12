//! # V4L2 Device Enumeration (Linux)
//!
//! Scans `/dev/video*` nodes and classifies each as a capture or output
//! device using `VIDIOC_QUERYCAP`.  Used by the GUI to populate V4L2 input
//! and output (v4l2loopback) selectors.

#![cfg(target_os = "linux")]

use std::path::PathBuf;

use v4l::capability::Flags;
use v4l::Device;

/// Information about one V4L2 device node.
#[derive(Debug, Clone)]
pub struct V4l2DeviceInfo {
    /// Path, e.g. `/dev/video10`.
    pub path: String,
    /// Numeric suffix from the path (`10` for `/dev/video10`). This is what
    /// nokhwa's `CameraIndex::Index` maps to on Linux.
    pub index: u32,
    /// Card/device name from `VIDIOC_QUERYCAP` (e.g. "UVC Camera" or
    /// "RustJay Output").
    pub card: String,
    /// Driver name (e.g. "uvcvideo", "v4l2 loopback").
    pub driver: String,
}

impl V4l2DeviceInfo {
    /// Human-friendly label for UI combos.
    pub fn display_name(&self) -> String {
        format!("{} ({})", self.card, self.path)
    }
}

/// Return all V4L2 nodes reporting `VIDEO_CAPTURE` capability.
///
/// Output-only v4l2loopback nodes (created with `exclusive_caps=1` under some
/// configs) are filtered out.
pub fn list_capture_devices() -> Vec<V4l2DeviceInfo> {
    scan_video_devices(Flags::VIDEO_CAPTURE)
}

/// Return all V4L2 nodes reporting `VIDEO_OUTPUT` capability — these are the
/// v4l2loopback virtual camera targets we can stream into.
pub fn list_output_devices() -> Vec<V4l2DeviceInfo> {
    scan_video_devices(Flags::VIDEO_OUTPUT)
}

fn scan_video_devices(required: Flags) -> Vec<V4l2DeviceInfo> {
    let mut results = Vec::new();

    let entries = match std::fs::read_dir("/dev") {
        Ok(e) => e,
        Err(e) => {
            log::warn!("[V4L2] Failed to read /dev: {}", e);
            return results;
        }
    };

    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("video") && n["video".len()..].chars().all(|c| c.is_ascii_digit()))
                .unwrap_or(false)
        })
        .collect();

    paths.sort();

    for path in paths {
        let path_str = match path.to_str() {
            Some(s) => s.to_string(),
            None => continue,
        };

        let index = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("video"))
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(u32::MAX);

        let device = match Device::with_path(&path) {
            Ok(d) => d,
            Err(e) => {
                log::debug!("[V4L2] Skipping {}: open failed ({})", path_str, e);
                continue;
            }
        };

        let caps = match device.query_caps() {
            Ok(c) => c,
            Err(e) => {
                log::debug!("[V4L2] Skipping {}: query_caps failed ({})", path_str, e);
                continue;
            }
        };

        if !caps.capabilities.contains(required) {
            continue;
        }

        results.push(V4l2DeviceInfo {
            path: path_str,
            index,
            card: caps.card,
            driver: caps.driver,
        });
    }

    results
}
