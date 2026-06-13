//! ProDJ Link integration.

use rustjay_core::{CdjDevice, ProDjState};

pub struct ProDjManager {
    client: prodjlink_rs::ProDjLinkClient,
    last_enabled: bool,
}

impl ProDjManager {
    pub fn new() -> Self {
        Self {
            client: prodjlink_rs::ProDjLinkClient::new(),
            last_enabled: false,
        }
    }

    /// Call once per frame from the main thread.
    pub fn update(&mut self, state: &mut ProDjState) {
        if state.enabled != self.last_enabled {
            self.last_enabled = state.enabled;
            log::info!(
                "[ProDJ] {}",
                if state.enabled { "enabled" } else { "disabled" }
            );
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

        // Read lock-free snapshot — zero acquisitions of ProDjLinkState's mutex
        let snap = self.client.snapshot();

        state.devices.clear();
        let mut master_bpm = 0.0f32;
        let mut has_master = false;

        for device in &snap.devices {
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

        if let Some(track) = &snap.current_track {
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

    /// Fallback: `prodjlink-rs` doesn't expose beat phase yet; replace when the API does.
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
