//! Ableton Link integration.

use rustjay_core::LinkState;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// Manages an Ableton Link session and copies live tempo data into
/// [`LinkState`] each frame.
///
/// Construct once and call [`update`](Self::update) every frame.
pub struct LinkManager {
    link: rusty_link::AblLink,
    shared: Arc<Shared>,
    last_enabled: bool,
    quantum: f64,
}

struct Shared {
    num_peers: AtomicUsize,
    is_playing: AtomicBool,
}

impl LinkManager {
    /// Create a new Link manager with an initial tempo of 120 BPM.
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            num_peers: AtomicUsize::new(0),
            is_playing: AtomicBool::new(false),
        });

        let mut link = rusty_link::AblLink::new(120.0);
        link.enable(false);
        link.enable_start_stop_sync(true);

        {
            let s = Arc::clone(&shared);
            link.set_num_peers_callback(move |n| {
                s.num_peers.store(n as usize, Ordering::Relaxed);
            });
        }
        {
            let s = Arc::clone(&shared);
            link.set_start_stop_callback(move |playing| {
                s.is_playing.store(playing, Ordering::Relaxed);
            });
        }

        Self {
            link,
            shared,
            last_enabled: false,
            quantum: 4.0,
        }
    }

    /// Explicitly disable the Link session.
    pub fn disable(&mut self) {
        self.link.enable(false);
        self.last_enabled = false;
    }

    /// Poll Link state and write it into `state`.
    ///
    /// Call this once per frame from the main thread.
    pub fn update(&mut self, state: &mut LinkState) {
        // Handle enable/disable transitions
        if state.enabled != self.last_enabled {
            self.link.enable(state.enabled);
            self.last_enabled = state.enabled;
            log::info!("[Link] {}", if state.enabled { "enabled" } else { "disabled" });
        }

        if state.quantum != self.quantum {
            self.quantum = state.quantum;
        }

        if !state.enabled {
            state.num_peers = 0;
            state.bpm = 0.0;
            state.beat_phase = 0.0;
            state.is_playing = false;
            return;
        }

        let mut session = rusty_link::SessionState::new();
        self.link.capture_app_session_state(&mut session);

        let time  = self.link.clock_micros();
        let bpm   = session.tempo();
        let phase = session.phase_at_time(time, self.quantum);

        // Normalize phase to 0–1
        let normalized_phase = if self.quantum > 0.0 {
            (phase / self.quantum).rem_euclid(1.0) as f32
        } else {
            0.0
        };

        state.bpm        = bpm as f32;
        state.beat_phase = normalized_phase;
        state.num_peers  = self.shared.num_peers.load(Ordering::Relaxed);
        state.is_playing = self.shared.is_playing.load(Ordering::Relaxed);
    }
}

impl Default for LinkManager {
    fn default() -> Self {
        Self::new()
    }
}
