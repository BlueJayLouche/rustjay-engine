//! Simple filesystem watcher for ISF shader hot-reload.
//!
//! Logs changed files; full reload wiring deferred to Phase 3/6 when deck
//! management UI is in place.

use notify::{Config, Error, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};

/// Watches the shaders directory for `.fs` changes.
pub struct ShaderWatcher {
    #[allow(dead_code)]
    watcher: RecommendedWatcher,
    rx: std::sync::Mutex<Receiver<Result<Event, Error>>>,
}

impl ShaderWatcher {
    /// Start watching `shaders_dir`.
    pub fn new(shaders_dir: &Path) -> anyhow::Result<Self> {
        let (tx, rx) = channel();
        let watcher = RecommendedWatcher::new(tx, Config::default())?;
        let mut this = Self {
            watcher,
            rx: std::sync::Mutex::new(rx),
        };
        this.watcher.watch(shaders_dir, RecursiveMode::NonRecursive)?;
        log::info!("[ShaderWatcher] watching {}", shaders_dir.display());
        Ok(this)
    }

    /// Poll for pending events. Call once per frame in `prepare`.
    pub fn poll(&self) -> Vec<Event> {
        let mut events = Vec::new();
        let rx = self.rx.lock().unwrap();
        while let Ok(result) = rx.try_recv() {
            match result {
                Ok(event) => {
                    if event
                        .paths
                        .iter()
                        .any(|p| p.extension().map(|e| e == "fs").unwrap_or(false))
                    {
                        events.push(event);
                    }
                }
                Err(e) => log::warn!("[ShaderWatcher] notify error: {}", e),
            }
        }
        events
    }
}
