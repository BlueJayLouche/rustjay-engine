//! Show file definition and settings.
//!
//! The root document type saved to `.qproj` files.

use serde::{Deserialize, Serialize};

pub mod migration;

/// Current file format version. Bumped when serialization schema changes.
pub const FILE_FORMAT_VERSION: i32 = 7;

/// Root of every QPlayer project file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShowFile {
    #[serde(default = "default_file_format_version")]
    pub file_format_version: i32,
    #[serde(default)]
    pub show_settings: ShowSettings,
    #[serde(default)]
    pub column_widths: Vec<f32>,
    #[serde(default)]
    pub cues: Vec<crate::Cue>,
}

impl Default for ShowFile {
    fn default() -> Self {
        Self {
            file_format_version: FILE_FORMAT_VERSION,
            show_settings: ShowSettings::default(),
            column_widths: Vec::new(),
            cues: Vec::new(),
        }
    }
}

fn default_file_format_version() -> i32 {
    FILE_FORMAT_VERSION
}

impl ShowFile {
    /// Choose the next QID for a new cue.
    ///
    /// If `after_qid` is provided, attempts decimal subdivision
    /// (e.g. 1 → 1.1 → 1.01). If all subdivisions are taken or no
    /// `after_qid` is given, falls back to `max(qid) + 1`.
    pub fn choose_qid(&self, after_qid: Option<rust_decimal::Decimal>) -> rust_decimal::Decimal {
        use rust_decimal::Decimal;

        // Collect existing QIDs for fast lookup
        let existing: std::collections::HashSet<Decimal> =
            self.cues.iter().map(|c| c.base().qid).collect();

        // Try decimal subdivision after the selected cue
        if let Some(base) = after_qid {
            for scale in [Decimal::from_str_exact("0.1").unwrap(),
                          Decimal::from_str_exact("0.01").unwrap(),
                          Decimal::from_str_exact("0.001").unwrap(),
                          Decimal::from_str_exact("0.0001").unwrap(),
                          Decimal::from_str_exact("0.00001").unwrap(),
                          Decimal::from_str_exact("0.000001").unwrap()] {
                let candidate = base + scale;
                if !existing.contains(&candidate) {
                    return candidate;
                }
            }
        }

        // Fallback: max + 1
        self.cues
            .iter()
            .map(|c| c.base().qid)
            .max()
            .unwrap_or(Decimal::ZERO)
            + Decimal::ONE
    }
}

/// Project-level settings (audio, networking, metadata).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShowSettings {
    #[serde(default = "default_title")]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default = "default_date")]
    pub date: chrono::NaiveDate,

    // Audio
    #[serde(default = "default_latency")]
    pub audio_latency: i32,
    #[serde(default)]
    pub exclusive_mode: bool,
    #[serde(default)]
    pub channel_offset: i32,
    #[serde(default)]
    pub audio_output_driver: AudioOutputDriver,
    #[serde(default)]
    pub audio_output_device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limiter: Option<AudioLimiterSettings>,

    // OSC
    #[serde(default)]
    pub osc_nic: String,
    #[serde(default = "default_osc_rx")]
    pub osc_rx_port: i32,
    #[serde(default = "default_osc_tx")]
    pub osc_tx_port: i32,

    // Remote control
    #[serde(default)]
    pub enable_remote_control: bool,
    #[serde(default = "default_true")]
    pub is_remote_host: bool,
    #[serde(default = "default_true")]
    pub sync_show_file_on_save: bool,
    #[serde(default = "default_true")]
    pub autosave_enabled: bool,
    #[serde(default = "default_node_name")]
    pub node_name: String,
    #[serde(default)]
    pub remote_nodes: Vec<RemoteNode>,

    // MSC
    #[serde(default)]
    pub enable_msc: bool,
    #[serde(default = "default_msc_port")]
    pub msc_rx_port: i32,
    #[serde(default = "default_msc_port")]
    pub msc_tx_port: i32,
    #[serde(default = "default_msc_device")]
    pub msc_rx_device: i32,
    #[serde(default = "default_msc_device_tx")]
    pub msc_tx_device: i32,
    #[serde(default = "default_negative_one")]
    pub msc_executor: i32,
    #[serde(default = "default_negative_one")]
    pub msc_page: i32,
}

impl Default for ShowSettings {
    fn default() -> Self {
        Self {
            title: default_title(),
            description: String::new(),
            author: String::new(),
            date: default_date(),
            audio_latency: default_latency(),
            exclusive_mode: false,
            channel_offset: 0,
            audio_output_driver: AudioOutputDriver::default(),
            audio_output_device: String::new(),
            limiter: None,
            osc_nic: String::new(),
            osc_rx_port: default_osc_rx(),
            osc_tx_port: default_osc_tx(),
            enable_remote_control: false,
            is_remote_host: true,
            sync_show_file_on_save: true,
            autosave_enabled: true,
            node_name: default_node_name(),
            remote_nodes: Vec::new(),
            enable_msc: false,
            msc_rx_port: default_msc_port(),
            msc_tx_port: default_msc_port(),
            msc_rx_device: default_msc_device(),
            msc_tx_device: default_msc_device_tx(),
            msc_executor: -1,
            msc_page: -1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RemoteNode {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub address: String,
    #[serde(skip)]
    pub last_seen: Option<std::time::Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AudioOutputDriver {
    #[default]
    WASAPI,
    Wave,
    DirectSound,
    ASIO,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct AudioLimiterSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub input_gain: f32,
    #[serde(default)]
    pub threshold: f32,
    #[serde(default)]
    pub attack: f32,
    #[serde(default)]
    pub release: f32,
}

// Default helpers
fn default_title() -> String {
    "Untitled".into()
}
fn default_date() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}
fn default_latency() -> i32 {
    10
}
fn default_osc_rx() -> i32 {
    9000
}
fn default_osc_tx() -> i32 {
    8000
}
fn default_true() -> bool {
    true
}
fn default_node_name() -> String {
    "QPlayer".into()
}
fn default_msc_port() -> i32 {
    6004
}
fn default_msc_device() -> i32 {
    0x70
}
fn default_msc_device_tx() -> i32 {
    0x71
}
fn default_negative_one() -> i32 {
    -1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_show_file_default_version() {
        let sf = ShowFile::default();
        assert_eq!(sf.file_format_version, FILE_FORMAT_VERSION);
    }

    #[test]
    fn test_show_settings_defaults() {
        let ss = ShowSettings::default();
        assert_eq!(ss.title, "Untitled");
        assert_eq!(ss.audio_latency, 10);
        assert_eq!(ss.osc_rx_port, 9000);
        assert!(!ss.enable_remote_control);
    }

    #[test]
    fn test_show_file_serde_roundtrip() {
        let sf = ShowFile {
            show_settings: ShowSettings {
                title: "My Show".into(),
                ..Default::default()
            },
            cues: vec![
                crate::Cue::Group {
                    base: crate::CueBase {
                        qid: rust_decimal::Decimal::from(1),
                        name: "Opening".into(),
                        ..Default::default()
                    },
                },
            ],
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&sf).unwrap();
        println!("{}", json);
        let de: ShowFile = serde_json::from_str(&json).unwrap();
        assert_eq!(sf, de);
    }
}
