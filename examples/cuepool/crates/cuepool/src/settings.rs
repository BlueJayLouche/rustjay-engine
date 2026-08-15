use cuepool_gui::SharedStateHandle;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct AppSettings {
    pub(crate) recent_files: Vec<std::path::PathBuf>,
    pub(crate) last_seen_release_notes: Option<String>,
}

pub(crate) const AUTOMATION_PROFILE_ENV: &str = "CUEPOOL_AUTOMATION_PROFILE";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AppProfile {
    Default,
    Automation(String),
}

impl AppProfile {
    pub(crate) fn from_env() -> Result<Self, String> {
        match std::env::var(AUTOMATION_PROFILE_ENV) {
            Ok(value) if value.is_empty() => Ok(Self::Default),
            Ok(value) if valid_profile_name(&value) => Ok(Self::Automation(value)),
            Ok(_) => Err(format!(
                "{AUTOMATION_PROFILE_ENV} must contain 1 to 64 lowercase letters, digits, or hyphens, start with a letter or digit, and not be a reserved Windows device name"
            )),
            Err(std::env::VarError::NotPresent) => Ok(Self::Default),
            Err(std::env::VarError::NotUnicode(_)) => {
                Err(format!("{AUTOMATION_PROFILE_ENV} must be valid Unicode"))
            }
        }
    }

    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Default => "default",
            Self::Automation(name) => name,
        }
    }

    pub(crate) fn lock_name(&self) -> String {
        let name = match self {
            Self::Default => "CuePool".to_string(),
            Self::Automation(name) => format!("CuePool-automation-{name}"),
        };
        #[cfg(unix)]
        return std::env::temp_dir()
            .join(format!("{name}.lock"))
            .to_string_lossy()
            .into_owned();
        #[cfg(not(unix))]
        return name;
    }

    pub(crate) fn settings_path(&self) -> Option<std::path::PathBuf> {
        dirs::config_dir().map(|root| self.path_in(root, "settings.json"))
    }

    pub(crate) fn persistent_log_path(&self) -> std::path::PathBuf {
        self.path_in(
            dirs::data_dir().unwrap_or_else(std::env::temp_dir),
            "cuepool.log",
        )
    }

    fn path_in(&self, root: std::path::PathBuf, filename: &str) -> std::path::PathBuf {
        let path = root.join("CuePool");
        match self {
            Self::Default => path.join(filename),
            Self::Automation(name) => path.join("automation").join(name).join(filename),
        }
    }
}

fn valid_profile_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !matches!(name, "con" | "prn" | "aux" | "nul")
        && !(name.len() == 4
            && matches!(&name[..3], "com" | "lpt")
            && matches!(name.as_bytes()[3], b'1'..=b'9'))
}

pub(crate) fn load_settings(profile: &AppProfile) -> AppSettings {
    if let Some(path) = profile.settings_path()
        && let Ok(data) = std::fs::read_to_string(&path)
        && let Ok(settings) = serde_json::from_str(&data)
    {
        return settings;
    }
    AppSettings::default()
}

pub(crate) fn save_settings(profile: &AppProfile, settings: &AppSettings) {
    if let Some(path) = profile.settings_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(data) = serde_json::to_string_pretty(settings) {
            let _ = std::fs::write(path, data);
        }
    }
}

pub(crate) fn save_settings_from_state(profile: &AppProfile, state: &SharedStateHandle) {
    let settings = state
        .lock()
        .map(|state| AppSettings {
            recent_files: state.recent_files.clone(),
            last_seen_release_notes: state.last_seen_release_notes.clone(),
        })
        .unwrap_or_default();
    save_settings(profile, &settings);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_leave_release_notes_unseen() {
        let settings: AppSettings = serde_json::from_str(r#"{"recent_files": []}"#).unwrap();

        assert_eq!(settings.last_seen_release_notes, None);
    }

    #[test]
    fn default_profile_keeps_existing_paths() {
        let root = std::path::PathBuf::from("root");
        let profile = AppProfile::Default;

        assert_eq!(profile.name(), "default");
        assert_eq!(
            profile.path_in(root.clone(), "settings.json"),
            root.join("CuePool").join("settings.json")
        );
        assert_eq!(
            profile.path_in(root, "cuepool.log"),
            std::path::PathBuf::from("root")
                .join("CuePool")
                .join("cuepool.log")
        );
    }

    #[test]
    fn automation_profiles_are_validated_and_isolated() {
        assert!(valid_profile_name("smoke-a"));
        for invalid in [
            "",
            "UPPER",
            "has_space",
            "../escape",
            "-leading",
            "con",
            "com1",
            "lpt9",
        ] {
            assert!(!valid_profile_name(invalid), "{invalid}");
        }

        let root = std::path::PathBuf::from("root");
        let first = AppProfile::Automation("smoke-a".into());
        let second = AppProfile::Automation("smoke-b".into());
        assert_ne!(
            first.path_in(root.clone(), "settings.json"),
            second.path_in(root.clone(), "settings.json")
        );
        assert_ne!(first.lock_name(), second.lock_name());
        assert_eq!(
            first.path_in(root, "cuepool.log"),
            std::path::PathBuf::from("root")
                .join("CuePool")
                .join("automation")
                .join("smoke-a")
                .join("cuepool.log")
        );
    }
}
