//! Persistence — `.varda/` workspace layout.
//!
//! - `scene.json`  — channels, decks, effects, modulation, crossfader, sequences
//! - `stage.json`  — surface layout, outputs, warp calibration
//! - `midi.json`   — MIDI controller mappings
//! - `keymap.json` — keyboard shortcut bindings
//! - `presets/`    — saved deck/channel presets
//!
//! See VARDA_PORT.md Phase 11.

use crate::scene::Scene;
use std::path::{Path, PathBuf};

/// Workspace loader/saver.
#[derive(Clone)]
pub struct Workspace {
    pub dir: PathBuf,
}

impl Default for Workspace {
    fn default() -> Self {
        default_workspace()
    }
}

impl Workspace {
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self { dir: dir.as_ref().to_path_buf() }
    }

    pub fn scene_path(&self) -> PathBuf {
        self.dir.join("scene.json")
    }

    pub fn stage_path(&self) -> PathBuf {
        self.dir.join("stage.json")
    }

    pub fn keymap_path(&self) -> PathBuf {
        self.dir.join("keymap.json")
    }

    pub fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)
    }

    pub fn save_scene(&self, scene: &Scene) -> anyhow::Result<()> {
        self.ensure_dir()?;
        let path = self.scene_path();
        let json = serde_json::to_string_pretty(scene)?;
        std::fs::write(&path, json)?;
        log::info!("[Workspace] scene saved to {}", path.display());
        Ok(())
    }

    pub fn load_scene(&self) -> anyhow::Result<Scene> {
        let path = self.scene_path();
        let json = std::fs::read_to_string(&path)?;
        let scene: Scene = serde_json::from_str(&json)?;
        log::info!("[Workspace] scene loaded from {}", path.display());
        Ok(scene)
    }

    #[cfg(feature = "projection")]
    pub fn save_stage(&self, stage: &crate::stage::VardaStage) -> anyhow::Result<()> {
        self.ensure_dir()?;
        let path = self.stage_path();
        let json = serde_json::to_string_pretty(stage)?;
        std::fs::write(&path, json)?;
        log::info!("[Workspace] stage saved to {}", path.display());
        Ok(())
    }

    #[cfg(feature = "projection")]
    pub fn load_stage(&self) -> anyhow::Result<crate::stage::VardaStage> {
        let path = self.stage_path();
        let json = std::fs::read_to_string(&path)?;
        let stage: crate::stage::VardaStage = serde_json::from_str(&json)?;
        log::info!("[Workspace] stage loaded from {}", path.display());
        Ok(stage)
    }

    pub fn save_keymap(&self, keymap: &crate::keymap::Keymap) -> anyhow::Result<()> {
        self.ensure_dir()?;
        let path = self.keymap_path();
        let json = serde_json::to_string_pretty(keymap)?;
        std::fs::write(&path, json)?;
        log::info!("[Workspace] keymap saved to {}", path.display());
        Ok(())
    }

    pub fn load_keymap(&self) -> anyhow::Result<crate::keymap::Keymap> {
        let path = self.keymap_path();
        let json = std::fs::read_to_string(&path)?;
        let keymap: crate::keymap::Keymap = serde_json::from_str(&json)?;
        log::info!("[Workspace] keymap loaded from {}", path.display());
        Ok(keymap)
    }

    pub fn exists(&self) -> bool {
        self.scene_path().exists()
    }
}

/// Default workspace path: `./.varda/` relative to CWD.
pub fn default_workspace() -> Workspace {
    Workspace::new(".varda")
}
