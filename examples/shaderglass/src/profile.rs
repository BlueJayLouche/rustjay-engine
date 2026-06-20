//! ShaderGlass profile: one JSON file capturing the whole look — which ISF
//! shader, its parameter values, and the input source.
//!
//! Params apply asynchronously: changing the shader re-registers descriptors a
//! frame or two later, so a loaded profile's params are stashed and applied
//! once the swap completes (see `ShaderTab::apply_pending_profile`).

use std::path::Path;

use rustjay_engine::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Profile {
    /// Ordered FX chain by library stem name, e.g. ["CRT-Glass", "ShaderBeam"].
    pub chain: Vec<String>,
    /// (prefixed param id, value) pairs. Excludes auto-driven `*sourceAspect`.
    pub params: Vec<(String, f32)>,
    /// Input source, best-effort restore.
    pub source: Option<SourceRef>,
}

#[derive(Serialize, Deserialize)]
pub struct SourceRef {
    /// "syphon" | "ndi" | "webcam".
    pub kind: String,
    pub name: String,
    pub device_index: Option<usize>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Profile {
    /// Snapshot the chain shader names, params and source from engine state.
    pub fn capture(chain_names: &[String], engine: &EngineState) -> Self {
        let params = engine
            .param_descriptors
            .iter()
            .filter(|d| !d.id.ends_with("sourceAspect")) // host-driven, not part of the look
            .map(|d| (d.id.clone(), engine.get_param(&d.id).unwrap_or(d.default)))
            .collect();

        Self {
            chain: chain_names.to_vec(),
            params,
            source: SourceRef::capture(engine),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }

    /// Issue the input command to restore the source. Syphon is best-effort:
    /// its uuid is discovered at runtime, so we can't replay it — log and skip.
    pub fn restore_source(&self, engine: &mut EngineState) {
        let Some(src) = &self.source else { return };
        match src.kind.as_str() {
            "webcam" => {
                engine.input_command = InputCommand::StartWebcam {
                    device_index: src.device_index.unwrap_or(0),
                    width: src.width.max(1),
                    height: src.height.max(1),
                    fps: src.fps.max(1),
                };
            }
            #[cfg(feature = "ndi")]
            "ndi" => {
                engine.input_command = InputCommand::StartNdi {
                    source_name: src.name.clone(),
                };
            }
            "syphon" => {
                log::info!(
                    "Profile source is Syphon '{}' — re-select it in the Input tab (uuid is runtime-only).",
                    src.name
                );
            }
            other => log::warn!("Profile source kind '{other}' not restorable on this build"),
        }
    }
}

impl SourceRef {
    fn capture(engine: &EngineState) -> Option<Self> {
        let inp = &engine.input;
        if !inp.is_active {
            return None;
        }
        let kind = match inp.input_type {
            InputType::Webcam => "webcam",
            #[cfg(feature = "ndi")]
            InputType::Ndi => "ndi",
            #[cfg(target_os = "macos")]
            InputType::Syphon => "syphon",
            _ => return None,
        };
        Some(Self {
            kind: kind.to_string(),
            name: inp.source_name.clone(),
            device_index: inp.device_index,
            width: inp.width,
            height: inp.height,
            fps: inp.fps as u32,
        })
    }
}
