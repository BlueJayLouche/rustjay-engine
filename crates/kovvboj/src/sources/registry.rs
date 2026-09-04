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
    /// Spout sender (Windows).
    Spout,
    /// SRT stream.
    Srt,
    /// HLS stream.
    Hls,
    /// DASH stream.
    Dash,
    /// RTMP stream.
    Rtmp,
    /// HTTP(S) stream.
    Http,
    /// RTSP stream.
    Rtsp,
}

/// Infer a stream source kind from a supported URL and require a host.
pub fn classify_stream_url(url: &str) -> Result<SourceKind, &'static str> {
    let url = url.trim();
    if url.is_empty() {
        return Err("Stream URL is required.");
    }

    let Some((scheme, remainder)) = url.split_once("://") else {
        return Err("Stream URL must include a scheme.");
    };
    let authority = remainder.split(['/', '?', '#']).next().unwrap_or_default();
    let host_and_port = authority.rsplit('@').next().unwrap_or_default();
    let host = if let Some(bracketed) = host_and_port.strip_prefix('[') {
        bracketed.split_once(']').map(|(host, _)| host).unwrap_or_default()
    } else {
        host_and_port.split(':').next().unwrap_or_default()
    };
    if host.trim().is_empty() {
        return Err("Stream URL must include a host.");
    }

    let scheme = scheme.to_ascii_lowercase();
    let path = remainder
        .split(['?', '#'])
        .next()
        .unwrap_or(remainder)
        .to_ascii_lowercase();
    match scheme.as_str() {
        "srt" => Ok(SourceKind::Srt),
        "rtmp" | "rtmps" => Ok(SourceKind::Rtmp),
        "rtsp" => Ok(SourceKind::Rtsp),
        "http" | "https" if path.ends_with(".m3u8") => Ok(SourceKind::Hls),
        "http" | "https" if path.ends_with(".mpd") => Ok(SourceKind::Dash),
        "http" | "https" => Ok(SourceKind::Http),
        _ => Err("Unsupported stream URL scheme."),
    }
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
                // Format: name|url (a legacy third kind field is ignored).
                let parts: Vec<&str> = line.split('|').collect();
                if parts.len() >= 2 {
                    let name = parts[0].trim().to_string();
                    let url = parts[1].trim().to_string();
                    let kind = match classify_stream_url(&url) {
                        Ok(kind) => kind,
                        Err(error) => {
                            log::warn!(
                                "Skipping stream '{}' from {}: {}",
                                name,
                                streams_path.display(),
                                error
                            );
                            continue;
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

        let mut registry = Self {
            shaders,
            images,
            videos,
            streams,
            builtins: Vec::new(),
        };
        // One code path for device discovery, so the generic NDI/Syphon/Spout
        // entries exist from startup and not only after a rescan.
        registry.refresh_builtins();
        registry
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
        #[cfg(target_os = "windows")]
        for (idx, info) in rustjay_io::SpoutDiscovery::list_senders()
            .into_iter()
            .enumerate()
        {
            builtins.push(SourceEntry {
                id: format!("spout_{}", idx),
                name: info.name.clone(),
                kind: SourceKind::Spout,
                path: None,
                device_index: 0,
            });
        }
        self.builtins = builtins;
        // Generic device entries, always present. A server or sender you have
        // not started yet cannot be discovered, so without these there is no
        // way to add the layer first and connect it later from the inspector.
        #[cfg(feature = "ndi")]
        self.builtins.push(SourceEntry {
            id: "ndi_any".to_string(),
            name: "NDI…".to_string(),
            kind: SourceKind::Ndi,
            path: None,
            device_index: 0,
        });
        #[cfg(target_os = "macos")]
        self.builtins.push(SourceEntry {
            id: "syphon_any".to_string(),
            name: "Syphon…".to_string(),
            kind: SourceKind::Syphon,
            path: None,
            device_index: 0,
        });
        #[cfg(target_os = "windows")]
        self.builtins.push(SourceEntry {
            id: "spout_any".to_string(),
            name: "Spout…".to_string(),
            kind: SourceKind::Spout,
            path: None,
            device_index: 0,
        });
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

#[cfg(test)]
mod tests {
    use super::{SourceKind, classify_stream_url};

    #[test]
    fn classifies_supported_stream_urls() {
        for (url, expected) in [
            ("srt://stream.example/live", SourceKind::Srt),
            ("rtmp://stream.example/live", SourceKind::Rtmp),
            ("rtmps://stream.example/live", SourceKind::Rtmp),
            ("http://stream.example/live.mp4", SourceKind::Http),
            ("HTTPS://stream.example/live", SourceKind::Http),
            ("https://stream.example/live.m3u8?token=1", SourceKind::Hls),
            ("https://stream.example/live.MPD#manifest", SourceKind::Dash),
            ("rtsp://stream.example/live", SourceKind::Rtsp),
            ("rtsp://[::1]:8554/live", SourceKind::Rtsp),
        ] {
            assert_eq!(classify_stream_url(url), Ok(expected), "{url}");
        }
    }

    #[test]
    fn rejects_invalid_stream_urls() {
        for url in [
            "",
            "stream.example/live",
            "ftp://stream.example/live",
            "http:///live",
            "rtsp://:8554/live",
            "srt://user@/live",
        ] {
            assert!(classify_stream_url(url).is_err(), "accepted {url}");
        }
    }
}

/// Copy a picked shader into the library folder, so that adding it once is
/// enough and it is there on the next launch.
///
/// A name that is already taken gets a numeric suffix, unless the file sitting
/// there is byte-identical: re-adding the same shader should not litter the
/// folder with copies, and adding a *different* shader that happens to share a
/// filename must not quietly load the bundled one instead.
pub fn install_shader(src: &Path, shaders_dir: &Path) -> std::io::Result<PathBuf> {
    if src.parent() == Some(shaders_dir) {
        return Ok(src.to_path_buf());
    }
    let contents = std::fs::read(src)?;
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("shader");
    std::fs::create_dir_all(shaders_dir)?;

    for attempt in 0..100 {
        let name = match attempt {
            0 => format!("{stem}.fs"),
            n => format!("{stem}_{n}.fs"),
        };
        let dest = shaders_dir.join(name);
        match std::fs::read(&dest) {
            // Already installed under this name — nothing to do.
            Ok(existing) if existing == contents => return Ok(dest),
            Ok(_) => continue,
            Err(_) => {
                std::fs::write(&dest, &contents)?;
                return Ok(dest);
            }
        }
    }
    Err(std::io::Error::other(format!(
        "100 different shaders are already installed as {stem}"
    )))
}

#[cfg(test)]
mod install_tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "kovvboj_install_{tag}_{}_{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn file(&self, name: &str, body: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, body).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn a_new_shader_lands_under_its_own_name() {
        let outside = TempDir::new("src_new");
        let library = TempDir::new("lib_new");
        let picked = outside.file("Oil_stain.fs", "// oil");

        let installed = install_shader(&picked, &library.0).unwrap();

        assert_eq!(installed, library.0.join("Oil_stain.fs"));
        assert_eq!(std::fs::read_to_string(&installed).unwrap(), "// oil");
    }

    #[test]
    fn adding_the_same_shader_twice_does_not_make_a_second_copy() {
        let outside = TempDir::new("src_same");
        let library = TempDir::new("lib_same");
        let picked = outside.file("Oil_stain.fs", "// oil");

        let first = install_shader(&picked, &library.0).unwrap();
        let second = install_shader(&picked, &library.0).unwrap();

        assert_eq!(first, second);
        assert_eq!(std::fs::read_dir(&library.0).unwrap().count(), 1);
    }

    #[test]
    fn a_different_shader_with_a_taken_name_is_kept_separate() {
        let outside = TempDir::new("src_clash");
        let library = TempDir::new("lib_clash");
        std::fs::write(library.0.join("Oil_stain.fs"), "// the bundled one").unwrap();
        let picked = outside.file("Oil_stain.fs", "// mine");

        let installed = install_shader(&picked, &library.0).unwrap();

        assert_eq!(installed, library.0.join("Oil_stain_1.fs"));
        assert_eq!(
            std::fs::read_to_string(library.0.join("Oil_stain.fs")).unwrap(),
            "// the bundled one"
        );
    }

    #[test]
    fn a_shader_already_in_the_library_is_left_where_it_is() {
        let library = TempDir::new("lib_inplace");
        let already = library.file("Oil_stain.fs", "// oil");

        let installed = install_shader(&already, &library.0).unwrap();

        assert_eq!(installed, already);
        assert_eq!(std::fs::read_dir(&library.0).unwrap().count(), 1);
    }
}
