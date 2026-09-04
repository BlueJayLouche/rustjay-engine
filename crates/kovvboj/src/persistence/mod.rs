//! Persistence — `.kovvboj/` workspace layout.
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
        Self {
            dir: dir.as_ref().to_path_buf(),
        }
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

    pub fn ui_path(&self) -> PathBuf {
        self.dir.join("ui.json")
    }

    /// Load UI preferences, falling back to defaults when absent or unreadable —
    /// a corrupt prefs file must not stop the app opening.
    pub fn load_ui(&self) -> UiPrefs {
        std::fs::read_to_string(self.ui_path())
            .ok()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }

    pub fn save_ui(&self, prefs: &UiPrefs) -> anyhow::Result<()> {
        self.ensure_dir()?;
        std::fs::write(self.ui_path(), serde_json::to_string_pretty(prefs)?)?;
        Ok(())
    }

    pub fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)
    }

    /// Where saved layers live, one JSON file each so they can be copied
    /// between workspaces by hand.
    pub fn layers_dir(&self) -> PathBuf {
        self.dir.join("layers")
    }

    pub fn favourites_path(&self) -> PathBuf {
        self.dir.join("favourites.json")
    }

    /// Ids of library entries the user starred. A missing or unreadable file
    /// just means none.
    pub fn load_favourites(&self) -> std::collections::HashSet<String> {
        std::fs::read_to_string(self.favourites_path())
            .ok()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }

    pub fn save_favourites(
        &self,
        favourites: &std::collections::HashSet<String>,
    ) -> anyhow::Result<()> {
        self.ensure_dir()?;
        // Sorted, so the file does not churn between runs for no reason.
        let mut ids: Vec<&String> = favourites.iter().collect();
        ids.sort();
        std::fs::write(
            self.favourites_path(),
            serde_json::to_string_pretty(&ids)?,
        )?;
        Ok(())
    }
}

#[cfg(feature = "mixer")]
impl Workspace {
    /// Write a saved layer, returning the file it landed in. The name is
    /// slugified so a layer called "Cam / Blur" cannot escape the directory.
    pub fn save_layer(&self, layer: &crate::scene::SavedLayer) -> anyhow::Result<PathBuf> {
        std::fs::create_dir_all(self.layers_dir())?;
        let slug: String = layer
            .name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
            .collect();
        let slug = slug.trim_matches('_').to_string();
        let slug = if slug.is_empty() { "layer".to_string() } else { slug };
        let path = self.layers_dir().join(format!("{slug}.json"));
        std::fs::write(&path, serde_json::to_string_pretty(layer)?)?;
        Ok(path)
    }

    /// Write a saved master chain. Same slug rules as a saved layer.
    pub fn save_chain(&self, chain: &crate::scene::SavedChain) -> anyhow::Result<PathBuf> {
        let dir = self.dir.join("chains");
        std::fs::create_dir_all(&dir)?;
        let slug: String = chain
            .name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
            .collect();
        let slug = slug.trim_matches('_').to_string();
        let slug = if slug.is_empty() { "chain".to_string() } else { slug };
        let path = dir.join(format!("{slug}.json"));
        std::fs::write(&path, serde_json::to_string_pretty(chain)?)?;
        Ok(path)
    }

    /// Write a saved group.
    pub fn save_group(&self, group: &crate::scene::SavedGroup) -> anyhow::Result<PathBuf> {
        let dir = self.dir.join("groups");
        std::fs::create_dir_all(&dir)?;
        let slug: String = group
            .name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
            .collect();
        let slug = slug.trim_matches('_').to_string();
        let slug = if slug.is_empty() { "group".to_string() } else { slug };
        let path = dir.join(format!("{slug}.json"));
        std::fs::write(&path, serde_json::to_string_pretty(group)?)?;
        Ok(path)
    }

    /// Every saved group on disk, name-sorted.
    pub fn load_groups(&self) -> Vec<crate::scene::SavedGroup> {
        let mut out: Vec<crate::scene::SavedGroup> = std::fs::read_dir(self.dir.join("groups"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
            .filter_map(|e| {
                let text = std::fs::read_to_string(e.path()).ok()?;
                match serde_json::from_str(&text) {
                    Ok(g) => Some(g),
                    Err(err) => {
                        log::warn!("[Groups] skipping {}: {err}", e.path().display());
                        None
                    }
                }
            })
            .collect();
        out.sort_by_key(|g| g.name.to_lowercase());
        out
    }

    /// Remove a saved group by name.
    pub fn delete_group(&self, name: &str) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(self.dir.join("groups"))?.flatten() {
            let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
            if let Ok(g) = serde_json::from_str::<crate::scene::SavedGroup>(&text)
                && g.name == name
            {
                std::fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    /// Every saved master chain on disk, name-sorted.
    pub fn load_chains(&self) -> Vec<crate::scene::SavedChain> {
        let mut out: Vec<crate::scene::SavedChain> = std::fs::read_dir(self.dir.join("chains"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
            .filter_map(|e| {
                let text = std::fs::read_to_string(e.path()).ok()?;
                match serde_json::from_str(&text) {
                    Ok(chain) => Some(chain),
                    Err(err) => {
                        log::warn!("[Chains] skipping {}: {err}", e.path().display());
                        None
                    }
                }
            })
            .collect();
        out.sort_by_key(|c| c.name.to_lowercase());
        out
    }

    /// Remove a saved chain by name.
    pub fn delete_chain(&self, name: &str) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(self.dir.join("chains"))?.flatten() {
            let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
            if let Ok(saved) = serde_json::from_str::<crate::scene::SavedChain>(&text)
                && saved.name == name
            {
                std::fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    /// Every saved layer on disk, name-sorted. Unreadable files are skipped
    /// rather than failing the whole listing.
    pub fn load_layers(&self) -> Vec<crate::scene::SavedLayer> {
        let mut out: Vec<crate::scene::SavedLayer> = std::fs::read_dir(self.layers_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
            .filter_map(|e| {
                let text = std::fs::read_to_string(e.path()).ok()?;
                match serde_json::from_str(&text) {
                    Ok(layer) => Some(layer),
                    Err(err) => {
                        log::warn!("[Layers] skipping {}: {err}", e.path().display());
                        None
                    }
                }
            })
            .collect();
        out.sort_by_key(|l| l.name.to_lowercase());
        out
    }

    /// Remove a saved layer by name, matching how it was written.
    pub fn delete_layer(&self, name: &str) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(self.layers_dir())?.flatten() {
            let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
            if let Ok(saved) = serde_json::from_str::<crate::scene::SavedLayer>(&text)
                && saved.name == name
            {
                std::fs::remove_file(entry.path())?;
            }
        }
        Ok(())
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
    pub fn save_stage(&self, stage: &crate::stage::KovvbojStage) -> anyhow::Result<()> {
        self.ensure_dir()?;
        let path = self.stage_path();
        let json = serde_json::to_string_pretty(stage)?;
        std::fs::write(&path, json)?;
        log::info!("[Workspace] stage saved to {}", path.display());
        Ok(())
    }

    #[cfg(feature = "projection")]
    pub fn load_stage(&self) -> anyhow::Result<crate::stage::KovvbojStage> {
        let path = self.stage_path();
        let json = std::fs::read_to_string(&path)?;
        let mut stage: crate::stage::KovvbojStage = serde_json::from_str(&json)?;
        stage.migrate_legacy_segments();
        stage.ensure_builtin_fixture_profiles();
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

/// Default workspace path: `./.kovvboj/` relative to CWD.
///
/// Falls back to a pre-rename `./.varda/` when it exists and `./.kovvboj/` does
/// not, so workspaces saved before the KOVVBOJ rename keep loading. Saving from
/// a legacy workspace keeps writing to `.varda/` — it is never migrated behind
/// the user's back.
/// UI preferences that outlive a session but are not part of the scene.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UiPrefs {
    /// Palette preset id — see `rustjay_gui::egui_theme::Palette::PRESETS`.
    pub palette: String,
    /// Width of the library panel.
    #[serde(default = "default_library_width")]
    pub library_width: f32,
    /// Width of the inspector panel.
    #[serde(default = "default_inspector_width")]
    pub inspector_width: f32,
    /// Built-in tabs left open as windows, by `GuiTab` name.
    ///
    /// Stored by name rather than index so adding a tab upstream cannot silently
    /// reopen the wrong window.
    #[serde(default)]
    pub open_windows: Vec<String>,
    /// Whether the Outputs window is showing.
    #[serde(default)]
    pub outputs_open: bool,
    /// Whether the Sequencer window is showing.
    #[serde(default)]
    pub sequencer_open: bool,
    /// Whether the inspector panel is showing.
    #[serde(default = "default_true")]
    pub inspector_open: bool,
    /// Whether the library panel is showing.
    #[serde(default = "default_true")]
    pub library_open: bool,
}

fn default_library_width() -> f32 {
    200.0
}

fn default_inspector_width() -> f32 {
    300.0
}

fn default_true() -> bool {
    true
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            palette: "kovvboj".to_string(),
            library_width: default_library_width(),
            inspector_width: default_inspector_width(),
            open_windows: Vec::new(),
            outputs_open: false,
            sequencer_open: false,
            inspector_open: true,
            library_open: true,
        }
    }
}

pub fn default_workspace() -> Workspace {
    // ponytail: read-only compatibility shim. Delete once no `.varda/` remains
    // in the wild; a real migration would have to move presets/ too.
    if !Path::new(".kovvboj").exists() && Path::new(".varda").exists() {
        return Workspace::new(".varda");
    }
    Workspace::new(".kovvboj")
}
