//! ProDJ Link integration.

use rustjay_core::{CdjDevice, ProDjState};

/// Manages a ProDJ Link client and copies discovered deck metadata into
/// [`ProDjState`] each frame.
///
/// Construct once and call [`update`](Self::update) every frame.
pub struct ProDjManager {
    client: prodjlink_rs::ProDjLinkClient,
    last_enabled: bool,
}

impl ProDjManager {
    /// Create a new ProDJ Link manager.
    pub fn new() -> Self {
        Self {
            client: prodjlink_rs::ProDjLinkClient::new(),
            last_enabled: false,
        }
    }

    /// Poll the ProDJ Link client and write discovered state into `state`.
    ///
    /// Call this once per frame from the main thread.
    pub fn update(&mut self, state: &mut ProDjState) {
        if state.enabled != self.last_enabled {
            self.last_enabled = state.enabled;
            log::info!("[ProDJ] {}", if state.enabled { "enabled" } else { "disabled" });
            if !state.enabled {
                state.devices.clear();
                state.master_bpm = 0.0;
                state.master_beat_phase = 0.0;
                state.current_track_artist.clear();
                state.current_track_title.clear();
                return;
            }
        }

        if !state.enabled {
            return;
        }

        // Refresh discovered devices
        state.devices.clear();
        let mut master_bpm = 0.0f32;
        let mut has_master = false;

        for device in self.client.cdj_devices() {
            let bpm = device.bpm.map(|b| b as f32).unwrap_or(0.0);
            if device.is_master {
                master_bpm = bpm;
                has_master = true;
            }
            state.devices.push(CdjDevice {
                device_id: device.device_id as u32,
                name: device.name.clone(),
                is_playing: device.is_playing,
                is_master: device.is_master,
                bpm: device.bpm.map(|b| b as f32),
            });
        }

        // If no explicit master, use the first playing deck's BPM
        if !has_master {
            for device in &state.devices {
                if device.is_playing {
                    master_bpm = device.bpm.unwrap_or(0.0);
                    break;
                }
            }
        }

        state.master_bpm = master_bpm;

        // ProDJ Link doesn't expose explicit beat phase in the current
        // prodjlink-rs API, so we derive a simple phase from time.
        // This is a best-effort approximation.
        state.master_beat_phase = Self::derive_beat_phase(master_bpm);

        // Update current master track metadata
        if let Some(track) = self.client.current_track() {
            state.current_track_artist.clone_from(&track.artist);
            state.current_track_title.clone_from(&track.title);
            // Prefer track BPM if master BPM is missing
            if state.master_bpm == 0.0 {
                if let Some(bpm) = track.bpm {
                    state.master_bpm = bpm as f32;
                }
            }
        } else {
            state.current_track_artist.clear();
            state.current_track_title.clear();
        }
    }

    /// Derive a beat phase from BPM using the system clock.
    ///
    /// This is a fallback because the current `prodjlink-rs` API does not
    /// expose explicit beat phase. When beat phase becomes available from
    /// the protocol, this should be replaced.
    fn derive_beat_phase(bpm: f32) -> f32 {
        if bpm <= 0.0 {
            return 0.0;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let beat_duration = 60.0 / bpm as f64;
        ((now / beat_duration) % 1.0) as f32
    }
}

impl Default for ProDjManager {
    fn default() -> Self {
        Self::new()
    }
}
