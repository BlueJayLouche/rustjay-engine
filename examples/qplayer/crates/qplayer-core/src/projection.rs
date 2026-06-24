//! Projection mapping configuration — canvas + output slices + edge blend.
//!
//! This is intentionally minimal and self-contained: one canvas, many outputs,
//! each sampling a rectangle from the canvas and rendering to its own window.

use serde::{Deserialize, Serialize};

/// How a video frame is fit into the projection canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CanvasFit {
    /// Scale to fill the entire canvas (may distort aspect ratio).
    Stretch,
    /// Scale to fit while preserving aspect ratio, letterboxing/pillarboxing.
    #[default]
    Fit,
}

/// Top-level projection state stored in a show file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionConfig {
    #[serde(default = "default_canvas_width")]
    pub canvas_width: u32,
    #[serde(default = "default_canvas_height")]
    pub canvas_height: u32,
    #[serde(default)]
    pub fit: CanvasFit,
    #[serde(default)]
    pub outputs: Vec<ProjectorOutput>,
}

impl ProjectionConfig {
    /// Default single-output config matching QPlayer's historical 1920×1080 video window.
    pub fn default_single() -> Self {
        Self {
            canvas_width: 1920,
            canvas_height: 1080,
            fit: CanvasFit::Fit,
            outputs: vec![ProjectorOutput::default_single()],
        }
    }

    /// 3×1 edge-blend preset matching `testFiles/example_3x1_EdgeBlend.png`.
    pub fn preset_3x1_edgeblend() -> Self {
        Self {
            canvas_width: 5400,
            canvas_height: 1080,
            fit: CanvasFit::Fit,
            outputs: vec![
                ProjectorOutput {
                    name: "Projector 1".into(),
                    source_x: 0,
                    source_y: 0,
                    source_width: 1920,
                    source_height: 1080,
                    output_width: 1920,
                    output_height: 1080,
                    fullscreen_monitor: None,
                    edge_blend: EdgeBlend {
                        right: EdgeBlendEdge { enabled: true, width: 180, gamma: 2.0 },
                        ..Default::default()
                    },
                },
                ProjectorOutput {
                    name: "Projector 2".into(),
                    source_x: 1740,
                    source_y: 0,
                    source_width: 1920,
                    source_height: 1080,
                    output_width: 1920,
                    output_height: 1080,
                    fullscreen_monitor: None,
                    edge_blend: EdgeBlend {
                        left: EdgeBlendEdge { enabled: true, width: 180, gamma: 2.0 },
                        right: EdgeBlendEdge { enabled: true, width: 180, gamma: 2.0 },
                        ..Default::default()
                    },
                },
                ProjectorOutput {
                    name: "Projector 3".into(),
                    source_x: 3480,
                    source_y: 0,
                    source_width: 1920,
                    source_height: 1080,
                    output_width: 1920,
                    output_height: 1080,
                    fullscreen_monitor: None,
                    edge_blend: EdgeBlend {
                        left: EdgeBlendEdge { enabled: true, width: 180, gamma: 2.0 },
                        ..Default::default()
                    },
                },
            ],
        }
    }
}

impl Default for ProjectionConfig {
    fn default() -> Self {
        Self::default_single()
    }
}

fn default_canvas_width() -> u32 {
    1920
}

fn default_canvas_height() -> u32 {
    1080
}

/// One projector output window sampling a rectangle from the canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectorOutput {
    #[serde(default = "default_name")]
    pub name: String,
    /// Source rectangle on the canvas, in pixels.
    #[serde(default)]
    pub source_x: u32,
    #[serde(default)]
    pub source_y: u32,
    #[serde(default = "default_1920")]
    pub source_width: u32,
    #[serde(default = "default_1080")]
    pub source_height: u32,
    /// Window/output resolution in pixels.
    #[serde(default = "default_1920")]
    pub output_width: u32,
    #[serde(default = "default_1080")]
    pub output_height: u32,
    /// Optional monitor index for borderless fullscreen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fullscreen_monitor: Option<usize>,
    #[serde(default)]
    pub edge_blend: EdgeBlend,
}

impl ProjectorOutput {
    pub fn default_single() -> Self {
        Self {
            name: default_name(),
            source_x: 0,
            source_y: 0,
            source_width: 1920,
            source_height: 1080,
            output_width: 1920,
            output_height: 1080,
            fullscreen_monitor: None,
            edge_blend: EdgeBlend::default(),
        }
    }
}

fn default_name() -> String {
    "Output".into()
}

fn default_1920() -> u32 {
    1920
}

fn default_1080() -> u32 {
    1080
}

/// Per-edge soft-edge blend parameters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct EdgeBlend {
    #[serde(default)]
    pub left: EdgeBlendEdge,
    #[serde(default)]
    pub right: EdgeBlendEdge,
    #[serde(default)]
    pub top: EdgeBlendEdge,
    #[serde(default)]
    pub bottom: EdgeBlendEdge,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EdgeBlendEdge {
    #[serde(default)]
    pub enabled: bool,
    /// Blend width in pixels.
    #[serde(default)]
    pub width: u32,
    /// Blend gamma (1.0 = linear, higher = steeper fade).
    #[serde(default = "default_gamma")]
    pub gamma: f32,
}

impl Default for EdgeBlendEdge {
    fn default() -> Self {
        Self {
            enabled: false,
            width: 0,
            gamma: default_gamma(),
        }
    }
}

fn default_gamma() -> f32 {
    2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = ProjectionConfig::default();
        assert_eq!(cfg.canvas_width, 1920);
        assert_eq!(cfg.canvas_height, 1080);
        assert_eq!(cfg.outputs.len(), 1);
    }

    #[test]
    fn test_3x1_preset() {
        let cfg = ProjectionConfig::preset_3x1_edgeblend();
        assert_eq!(cfg.canvas_width, 5400);
        assert_eq!(cfg.outputs.len(), 3);
        assert!(cfg.outputs[0].edge_blend.right.enabled);
        assert!(cfg.outputs[1].edge_blend.left.enabled);
        assert!(cfg.outputs[1].edge_blend.right.enabled);
        assert!(cfg.outputs[2].edge_blend.left.enabled);
    }

    #[test]
    fn test_serde_roundtrip() {
        let cfg = ProjectionConfig::preset_3x1_edgeblend();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        let de: ProjectionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, de);
    }
}
