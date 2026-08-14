#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct AppSettings {
    pub(crate) recent_files: Vec<std::path::PathBuf>,
}

fn settings_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|p| p.join("CuePool").join("settings.json"))
}

pub(crate) fn load_settings() -> AppSettings {
    if let Some(path) = settings_path()
        && let Ok(data) = std::fs::read_to_string(&path)
        && let Ok(settings) = serde_json::from_str(&data)
    {
        return settings;
    }
    AppSettings::default()
}

pub(crate) fn save_settings(settings: &AppSettings) {
    if let Some(path) = settings_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(data) = serde_json::to_string_pretty(settings) {
            let _ = std::fs::write(path, data);
        }
    }
}
