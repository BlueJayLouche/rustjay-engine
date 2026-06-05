//! Source / effect registry — enumerates available shaders, images, and sources.
//!
//! Drives the Library panel and API enumeration (T02.4).

use std::path::{Path, PathBuf};

/// One entry in the source library.
#[derive(Debug, Clone)]
pub struct SourceEntry {
    /// Stable identifier (filename or UUID).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// What kind of source this is.
    pub kind: SourceKind,
    /// Absolute path, if applicable.
    pub path: Option<PathBuf>,
}

/// Classification of a source entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Built-in generators (solid color, camera, etc.).
    pub builtins: Vec<SourceEntry>,
}

impl Registry {
    /// Scan the given directories for sources.
    pub fn scan(shaders_dir: &Path, _assets_dir: &Path) -> Self {
        let mut shaders = Vec::new();
        let images = Vec::new();
        let videos = Vec::new();

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
                    });
                }
            }
        }

        // Sort for deterministic ordering.
        shaders.sort_by(|a, b| a.name.cmp(&b.name));

        Self {
            shaders,
            images,
            videos,
            builtins: vec![
                SourceEntry {
                    id: "solid_color".to_string(),
                    name: "Solid Color".to_string(),
                    kind: SourceKind::SolidColor,
                    path: None,
                },
                SourceEntry {
                    id: "camera".to_string(),
                    name: "Camera".to_string(),
                    kind: SourceKind::Camera,
                    path: None,
                },
            ],
        }
    }

    /// All entries flattened.
    pub fn all(&self) -> Vec<&SourceEntry> {
        self.shaders
            .iter()
            .chain(&self.images)
            .chain(&self.videos)
            .chain(&self.builtins)
            .collect()
    }
}
