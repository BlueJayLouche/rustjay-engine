use cuepool_gui::SharedStateHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Autosave background thread: writes dirty show file to rotating backups every 60 s.
pub(crate) fn spawn_autosave_thread(state: SharedStateHandle, running: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut slot = 0usize;
        let mut elapsed = 0u64;
        while running.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1));
            if !running.load(Ordering::Relaxed) {
                break;
            }
            elapsed += 1;
            if elapsed < 60 {
                continue;
            }
            elapsed = 0;
            let (should_save, path, autosave_enabled) = {
                let Ok(state) = state.lock() else { continue };
                (state.dirty, state.project_path.clone(), state.show_file.show_settings.autosave_enabled)
            };
            if !autosave_enabled || !should_save {
                continue;
            }
            let Some(_project_path) = path else { continue };

            let dir = dirs::data_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("CuePool");
            if let Err(e) = std::fs::create_dir_all(&dir) {
                log::warn!("Autosave: failed to create dir {:?}: {}", dir, e);
                continue;
            }

            slot = (slot % 5) + 1;
            let backup_path = dir.join(format!("autoback_{}.qproj", slot));
            let json = {
                let Ok(state) = state.lock() else { continue };
                match serde_json::to_string_pretty(&state.show_file) {
                    Ok(j) => j,
                    Err(e) => {
                        log::warn!("Autosave: serialization failed: {}", e);
                        continue;
                    }
                }
            };
            if let Err(e) = std::fs::write(&backup_path, json) {
                log::warn!("Autosave: failed to write {:?}: {}", backup_path, e);
            } else {
                log::info!("Autosaved to {:?}", backup_path);
            }
        }
    });
}

/// Attempt an emergency save before the process exits.
pub(crate) fn emergency_save(state: &SharedStateHandle) {
    let (json, path) = {
        let Ok(state) = state.lock() else { return };
        let json = match serde_json::to_string_pretty(&state.show_file) {
            Ok(j) => j,
            Err(e) => {
                log::error!("Emergency save: serialization failed: {}", e);
                return;
            }
        };
        (json, state.project_path.clone())
    };

    let dir = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("CuePool");
    let _ = std::fs::create_dir_all(&dir);

    // Prefer crash_recovery.qproj, but if a project_path exists, also save there
    let crash_path = dir.join("crash_recovery.qproj");
    if let Err(e) = std::fs::write(&crash_path, &json) {
        log::error!("Emergency save: failed to write {:?}: {}", crash_path, e);
    } else {
        log::info!("Emergency save written to {:?}", crash_path);
    }

    if let Some(project_path) = path {
        if let Err(e) = std::fs::write(&project_path, &json) {
            log::error!("Emergency save: failed to overwrite {:?}: {}", project_path, e);
        } else {
            log::info!("Emergency save overwritten {:?}", project_path);
        }
    }
}
