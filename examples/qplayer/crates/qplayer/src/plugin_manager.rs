//! Plugin manager — scans `plugins/` directory and manages loaded WASM plugins.

use qplayer_plugin_api::{PluginHost, PluginInstance};
use std::path::Path;

pub struct PluginInfo {
    pub name: String,
    pub path: String,
}

pub struct PluginManager {
    _host: PluginHost,
    plugins: Vec<PluginInstance>,
    plugin_info: Vec<PluginInfo>,
}

impl PluginManager {
    pub fn new() -> anyhow::Result<Self> {
        let host = PluginHost::new()?;
        Ok(Self {
            _host: host,
            plugins: Vec::new(),
            plugin_info: Vec::new(),
        })
    }

    /// Scan a directory for `.wasm` files and load them.
    pub fn load_from_dir(&mut self, dir: &Path) {
        if !dir.exists() {
            log::debug!("Plugin directory {:?} does not exist, skipping.", dir);
            return;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("Failed to read plugin directory {:?}: {}", dir, e);
                return;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "wasm").unwrap_or(false) {
                self.load_plugin(&path);
            }
        }
    }

    fn load_plugin(&mut self, path: &Path) {
        // We need a fresh host to load each plugin because PluginHost::load
        // borrows self. In practice we can create a temporary engine.
        match PluginHost::new() {
            Ok(host) => match host.load(path) {
                Ok(mut plugin) => {
                    let name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown");
                    if let Err(e) = plugin.on_load() {
                        log::warn!("Plugin '{}' on_load failed: {}", name, e);
                    } else {
                        log::info!("Loaded plugin: {}", name);
                        self.plugins.push(plugin);
                        self.plugin_info.push(PluginInfo {
                            name: name.to_string(),
                            path: path.to_string_lossy().to_string(),
                        });
                    }
                }
                Err(e) => {
                    log::warn!(
                        "Failed to load plugin {:?}: {}",
                        path.file_stem().unwrap_or_default(),
                        e
                    );
                }
            },
            Err(e) => {
                log::warn!("Failed to create plugin host for {:?}: {}", path, e);
            }
        }
    }

    pub fn on_go(&mut self, cue_qid: i32) {
        for plugin in &mut self.plugins {
            if let Err(e) = plugin.on_go(cue_qid) {
                log::warn!("Plugin on_go failed: {}", e);
            }
        }
    }

    pub fn on_save(&mut self) {
        for plugin in &mut self.plugins {
            if let Err(e) = plugin.on_save() {
                log::warn!("Plugin on_save failed: {}", e);
            }
        }
    }

    pub fn on_unload(&mut self) {
        for plugin in &mut self.plugins {
            if let Err(e) = plugin.on_unload() {
                log::warn!("Plugin on_unload failed: {}", e);
            }
        }
    }

    pub fn on_slow_update(&mut self) {
        for plugin in &mut self.plugins {
            if let Err(e) = plugin.on_slow_update() {
                log::warn!("Plugin on_slow_update failed: {}", e);
            }
        }
    }

    pub fn list_plugins(&self) -> &[PluginInfo] {
        &self.plugin_info
    }
}
