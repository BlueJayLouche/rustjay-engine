//! Source / effect registry — enumerates available shaders, images, and sources.
//!
//! Drives the Library panel and API enumeration (T02.4).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One entry in the source library.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceEntry {
    /// Stable identifier (filename or UUID).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// What kind of source this is.
    pub kind: SourceKind,
    /// Absolute path, if applicable.
    pub path: Option<PathBuf>,
    /// Device index for camera/NDI sources.
    pub device_index: usize,
}

/// Classification of a source entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    /// ISF shader (generator or filter).
    Isf,
    /// Static image.
    Image,
    /// Video file.
    Video,
    /// Solid color generator.
    SolidColor,
    /// Live camera.
    Camera,
    /// NDI stream.
    Ndi,
    /// Syphon server (macOS).
    Syphon,
    /// SRT stream.
    Srt,
    /// HLS stream.
    Hls,
    /// DASH stream.
    Dash,
    /// RTMP stream.
    Rtmp,
}

/// Registry of available sources and effects.
#[derive(Default)]
pub struct Registry {
    /// ISF shaders discovered on disk.
    pub shaders: Vec<SourceEntry>,
    /// Images discovered on disk.
    pub images: Vec<SourceEntry>,
    /// Videos discovered on disk.
    pub videos: Vec<SourceEntry>,
    /// Live stream URLs (loaded from assets/streams.txt).
    pub streams: Vec<SourceEntry>,
    /// Built-in generators (solid color, camera, etc.).
    pub builtins: Vec<SourceEntry>,
}

impl Registry {
    /// Scan the given directories for sources.
    pub fn scan(shaders_dir: &Path, assets_dir: &Path) -> Self {
        let mut shaders = Vec::new();
        let mut images = Vec::new();
        let mut videos = Vec::new();

        // Scan ISF shaders
        if let Ok(entries) = std::fs::read_dir(shaders_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "fs").unwrap_or(false) {
                    let name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let id = name.to_lowercase().replace(' ', "_");
                    shaders.push(SourceEntry {
                        id,
                        name,
                        kind: SourceKind::Isf,
                        path: Some(path),
                        device_index: 0,
                    });
                }
            }
        }

        // Sort for deterministic ordering.
        shaders.sort_by(|a, b| a.name.cmp(&b.name));

        // Scan images and videos in assets_dir
        if let Ok(entries) = std::fs::read_dir(assets_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    let ext_lower = ext.to_lowercase();
                    let name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let id = name.to_lowercase().replace(' ', "_");
                    match ext_lower.as_str() {
                        "png" | "jpg" | "jpeg" => {
                            images.push(SourceEntry {
                                id,
                                name,
                                kind: SourceKind::Image,
                                path: Some(path),
                                device_index: 0,
                            });
                        }
                        "mp4" | "mov" | "avi" | "mkv" | "webm" => {
                            videos.push(SourceEntry {
                                id,
                                name,
                                kind: SourceKind::Video,
                                path: Some(path),
                                device_index: 0,
                            });
                        }
                        _ => {}
                    }
                }
            }
        }
        images.sort_by(|a, b| a.name.cmp(&b.name));
        videos.sort_by(|a, b| a.name.cmp(&b.name));

        // Load stream URLs from assets/streams.txt if present.
        let mut streams = Vec::new();
        let streams_path = assets_dir.join("streams.txt");
        if let Ok(content) = std::fs::read_to_string(&streams_path) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                // Format: name|url|kind  (kind = srt, hls, dash, rtmp)
                let parts: Vec<&str> = line.split('|').collect();
                if parts.len() >= 2 {
                    let name = parts[0].trim().to_string();
                    let url = parts[1].trim().to_string();
                    let kind_str = parts.get(2).map(|s| s.trim()).unwrap_or("");
                    let kind = match kind_str.to_lowercase().as_str() {
                        "srt" => SourceKind::Srt,
                        "hls" => SourceKind::Hls,
                        "dash" => SourceKind::Dash,
                        "rtmp" | "rtmps" => SourceKind::Rtmp,
                        _ => {
                            // Auto-detect from URL prefix.
                            if url.starts_with("srt://") {
                                SourceKind::Srt
                            } else if url.starts_with("rtmp://") || url.starts_with("rtmps://") {
                                SourceKind::Rtmp
                            } else if url.contains(".m3u8") || url.contains("/hls") {
                                SourceKind::Hls
                            } else if url.contains(".mpd") || url.contains("/dash") {
                                SourceKind::Dash
                            } else {
                                SourceKind::Rtmp
                            }
                        }
                    };
                    let id = name.to_lowercase().replace(' ', "_");
                    streams.push(SourceEntry {
                        id,
                        name,
                        kind,
                        path: Some(std::path::PathBuf::from(&url)),
                        device_index: 0,
                    });
                }
            }
        }

        Self {
            shaders,
            images,
            videos,
            streams,
            builtins: {
                let mut builtins = vec![
                    SourceEntry {
                        id: "solid_color".to_string(),
                        name: "Solid Color".to_string(),
                        kind: SourceKind::SolidColor,
                        path: None,
                        device_index: 0,
                    },
                ];
                // Discover webcams via the engine's nokhwa backend.
                #[cfg(feature = "webcam")]
                for (idx, name) in rustjay_io::list_cameras().into_iter().enumerate() {
                    let id = format!("camera_{}", idx);
                    builtins.push(SourceEntry {
                        id: id.clone(),
                        name,
                        kind: SourceKind::Camera,
                        path: None,
                        device_index: idx,
                    });
                }
                #[cfg(feature = "ndi")]
                {
                    for (idx, name) in rustjay_io::list_ndi_sources(500).into_iter().enumerate() {
                        let id = format!("ndi_{}", idx);
                        builtins.push(SourceEntry {
                            id: id.clone(),
                            name: name.clone(),
                            kind: SourceKind::Ndi,
                            path: None,
                            device_index: 0,
                        });
                    }
                }
                #[cfg(target_os = "macos")]
                {
                    for (idx, info) in rustjay_io::SyphonDiscovery::new()
                        .discover_servers()
                        .into_iter()
                        .enumerate()
                    {
                        let id = format!("syphon_{}", idx);
                        builtins.push(SourceEntry {
                            id: id.clone(),
                            name: info.name.clone(),
                            kind: SourceKind::Syphon,
                            path: Some(std::path::PathBuf::from(&info.uuid)),
                            device_index: 0,
                        });
                    }
                }
                builtins
            },
        }
    }

    /// Re-scan live devices (cameras, NDI, Syphon) without touching shaders/images/videos.
    pub fn refresh_builtins(&mut self) {
        let mut builtins = vec![SourceEntry {
            id: "solid_color".to_string(),
            name: "Solid Color".to_string(),
            kind: SourceKind::SolidColor,
            path: None,
            device_index: 0,
        }];
        #[cfg(feature = "webcam")]
        for (idx, name) in rustjay_io::list_cameras().into_iter().enumerate() {
            builtins.push(SourceEntry {
                id: format!("camera_{}", idx),
                name,
                kind: SourceKind::Camera,
                path: None,
                device_index: idx,
            });
        }
        #[cfg(feature = "ndi")]
        for (idx, name) in rustjay_io::list_ndi_sources(500).into_iter().enumerate() {
            builtins.push(SourceEntry {
                id: format!("ndi_{}", idx),
                name,
                kind: SourceKind::Ndi,
                path: None,
                device_index: 0,
            });
        }
        #[cfg(target_os = "macos")]
        for (idx, info) in rustjay_io::SyphonDiscovery::new()
            .discover_servers()
            .into_iter()
            .enumerate()
        {
            builtins.push(SourceEntry {
                id: format!("syphon_{}", idx),
                name: info.name.clone(),
                kind: SourceKind::Syphon,
                path: Some(std::path::PathBuf::from(&info.uuid)),
                device_index: 0,
            });
        }
        self.builtins = builtins;
    }

    /// All entries flattened.
    pub fn all(&self) -> Vec<&SourceEntry> {
        self.shaders
            .iter()
            .chain(&self.images)
            .chain(&self.videos)
            .chain(&self.streams)
            .chain(&self.builtins)
            .collect()
    }
}
