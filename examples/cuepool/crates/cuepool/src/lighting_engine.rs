//! Lighting cue playback — crossfades fixture looks and streams DMX.
//!
//! One theatrical crossfader: `go()` captures the *current* live state
//! (mid-fade included) as the fade source and the cue snapshot merged over it
//! as the target. Fixtures absent from a cue's snapshot track their current
//! state (LTP). A paced [`DmxSender`] thread handles wire pacing/keep-alive;
//! this engine only submits frames when something changed.

use std::collections::BTreeMap;
use std::time::Instant;

use cuepool_core::lighting::{
    render_look, FixtureId, FixtureLook, LightingConfig, LightingProtocol,
};
use cuepool_core::FadeType;
use rustjay_lighting::{
    color_pipeline, demux_tile, pack_fixtures, ArtNetTransport, Dest, DmxFrame, DmxSender,
    DmxTransport, SacnTransport, DMX_UNIVERSE_SIZE,
};

struct ActiveFade {
    start: Instant,
    duration: f32,
    fade_type: FadeType,
    from: BTreeMap<FixtureId, FixtureLook>,
    to: BTreeMap<FixtureId, FixtureLook>,
}

#[derive(Default)]
pub struct LightingEngine {
    sender: Option<DmxSender>,
    /// (protocol, dest_ip, fps bits) the sender was built from.
    applied: Option<(LightingProtocol, String, u32)>,
    live: BTreeMap<FixtureId, FixtureLook>,
    fade: Option<ActiveFade>,
    /// Latest sampled canvas pixels per pixel-map segment: (cols, rows, RGBA).
    segment_pixels: BTreeMap<u32, (u32, u32, Vec<u8>)>,
    /// Live state changed since the last submit.
    dirty: bool,
    last_tick: Option<Instant>,
}

/// Same curve family as the audio `FadeProcessor`.
fn curve(t: f32, fade_type: FadeType) -> f32 {
    match fade_type {
        FadeType::Linear => t,
        FadeType::Square => t * t,
        FadeType::InverseSquare => t.sqrt(),
        FadeType::SCurve => t * t * (3.0 - 2.0 * t),
    }
}

impl LightingEngine {
    /// Fire a lighting cue: crossfade live state to `snapshot` over `fade_time`.
    pub fn go(
        &mut self,
        snapshot: &BTreeMap<FixtureId, FixtureLook>,
        fade_time: f32,
        fade_type: FadeType,
    ) {
        let from = self.evaluate(Instant::now());
        let mut to = from.clone();
        for (id, look) in snapshot {
            to.insert(*id, *look);
        }
        if fade_time > 0.0 {
            self.fade = Some(ActiveFade {
                start: Instant::now(),
                duration: fade_time,
                fade_type,
                from,
                to,
            });
        } else {
            self.live = to;
            self.fade = None;
        }
        self.dirty = true;
    }

    /// Feed freshly sampled canvas pixels for one pixel-map segment.
    pub fn set_segment_pixels(&mut self, id: u32, cols: u32, rows: u32, rgba: Vec<u8>) {
        self.segment_pixels.insert(id, (cols, rows, rgba));
        self.dirty = true;
    }

    /// Cancel an in-flight fade, holding the current mid-fade state.
    pub fn stop_fade(&mut self) {
        if self.fade.is_some() {
            self.live = self.evaluate(Instant::now());
            self.fade = None;
            self.dirty = true;
        }
    }

    /// Current looks at `now` — mid-fade values while a fade is running.
    fn evaluate(&self, now: Instant) -> BTreeMap<FixtureId, FixtureLook> {
        match &self.fade {
            None => self.live.clone(),
            Some(f) => {
                let t = if f.duration > 0.0 {
                    (now.duration_since(f.start).as_secs_f32() / f.duration).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let t = curve(t, f.fade_type);
                f.to
                    .iter()
                    .map(|(id, target)| {
                        let start = f.from.get(id).copied().unwrap_or_default();
                        (*id, start.lerp(target, t))
                    })
                    .collect()
            }
        }
    }

    /// Periodic tick from the main loop: manages the sender lifecycle, advances
    /// fades, and submits a frame when the state changed. Self-throttled.
    pub fn tick(&mut self, cfg: &LightingConfig) {
        // ~60 Hz cap — about_to_wait runs far hotter under ControlFlow::Poll.
        let now = Instant::now();
        if let Some(last) = self.last_tick {
            if now.duration_since(last).as_secs_f32() < 1.0 / 60.0 {
                return;
            }
        }
        self.last_tick = Some(now);

        self.reconcile_sender(cfg);
        let Some(sender) = &self.sender else { return };

        let fading = self.fade.is_some();
        if !fading && !self.dirty {
            return; // DmxSender keep-alive re-sends the latest frame.
        }

        let looks = self.evaluate(now);
        // Fade complete → collapse into live state.
        if let Some(f) = &self.fade {
            if now.duration_since(f.start).as_secs_f32() >= f.duration {
                self.live = f.to.clone();
                self.fade = None;
            }
        }

        let mut frame = DmxFrame::new();
        for fixture in &cfg.fixtures {
            let Some(profile) = cfg.profile(&fixture.profile_id) else {
                continue;
            };
            let look = looks.get(&fixture.id).copied().unwrap_or_default();
            let bytes = render_look(&profile, &look);
            let u = frame.universe_mut(fixture.universe);
            let start = fixture.address.max(1) as usize - 1;
            for (i, b) in bytes.iter().enumerate() {
                if start + i < DMX_UNIVERSE_SIZE {
                    u[start + i] = *b;
                }
            }
        }

        // Pixel-map segments overlay their channels over cue looks.
        self.segment_pixels
            .retain(|id, _| cfg.segments.iter().any(|s| s.id == *id));
        for seg in cfg.active_segments() {
            let Some((cols, rows, rgba)) = self.segment_pixels.get(&seg.id) else {
                continue;
            };
            let Some(profile) = cfg.profile(&seg.profile_id) else {
                continue;
            };
            let pixels = demux_tile(rgba, *cols, [0, 0], [*cols, *rows], seg.order);
            let mut bytes = Vec::with_capacity(pixels.len() * profile.footprint());
            for p in pixels {
                // Sampler delivers RGBA; the colour pipeline expects BGRA.
                let bgra = [p[2], p[1], p[0], p[3]];
                bytes.extend(color_pipeline(bgra, seg.gamma, &seg.color, &profile));
            }
            pack_fixtures(&mut frame, profile.footprint(), &bytes, seg.universe, seg.address);
        }

        sender.submit(frame);
        self.dirty = false;
    }

    /// Build/tear down/rebuild the DMX sender to match the config.
    fn reconcile_sender(&mut self, cfg: &LightingConfig) {
        let wanted = cfg
            .enabled
            .then(|| (cfg.protocol, cfg.dest_ip.clone(), cfg.fps.to_bits()));
        if wanted == self.applied {
            return;
        }
        if let Some(sender) = self.sender.take() {
            sender.shutdown();
        }
        self.applied = wanted.clone();
        let Some((protocol, dest_ip, _)) = wanted else {
            return;
        };

        let dest = match dest_ip.trim() {
            "" => match protocol {
                LightingProtocol::Sacn => Dest::Multicast,
                LightingProtocol::ArtNet => Dest::Broadcast,
            },
            ip => match ip.parse() {
                Ok(addr) => Dest::Unicast(addr),
                Err(_) => {
                    log::error!("Lighting: invalid destination IP '{ip}'");
                    return;
                }
            },
        };
        log::info!("Lighting output: {protocol:?} → {dest:?} @ {} fps", cfg.fps);
        let transport: std::io::Result<Box<dyn DmxTransport>> = match protocol {
            LightingProtocol::Sacn => {
                SacnTransport::new(dest, 100, "CuePool").map(|t| Box::new(t) as _)
            }
            LightingProtocol::ArtNet => ArtNetTransport::new(dest).map(|t| Box::new(t) as _),
        };
        match transport {
            Ok(t) => {
                self.sender = Some(DmxSender::spawn(t, cfg.fps));
                self.dirty = true; // push current state to the new output
            }
            Err(e) => log::error!("Lighting output failed to start: {e}"),
        }
    }

    /// Drop the sender (project close/reload). Live state resets.
    pub fn shutdown(&mut self) {
        if let Some(sender) = self.sender.take() {
            sender.shutdown();
        }
        self.applied = None;
        self.live.clear();
        self.fade = None;
        self.segment_pixels.clear();
        self.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(dimmer: f32) -> FixtureLook {
        FixtureLook { dimmer, ..Default::default() }
    }

    #[test]
    fn go_snap_and_fade_evaluate() {
        let mut eng = LightingEngine::default();
        // Snap (fade 0): live is the snapshot immediately.
        let mut snap = BTreeMap::new();
        snap.insert(1, look(1.0));
        eng.go(&snap, 0.0, FadeType::Linear);
        assert_eq!(eng.evaluate(Instant::now()).get(&1).unwrap().dimmer, 1.0);

        // Fade to 0 over 10s: shortly after go, still near 1.0.
        let mut snap2 = BTreeMap::new();
        snap2.insert(1, look(0.0));
        eng.go(&snap2, 10.0, FadeType::Linear);
        let early = eng.evaluate(Instant::now()).get(&1).unwrap().dimmer;
        assert!(early > 0.9, "fade barely started, got {early}");
        // Evaluating at fade end lands on the target.
        let end = eng
            .evaluate(Instant::now() + std::time::Duration::from_secs(11))
            .get(&1)
            .unwrap()
            .dimmer;
        assert_eq!(end, 0.0);
    }

    #[test]
    fn absent_fixture_tracks_through_cue() {
        let mut eng = LightingEngine::default();
        let mut a = BTreeMap::new();
        a.insert(1, look(0.8));
        eng.go(&a, 0.0, FadeType::Linear);
        // Second cue only touches fixture 2 — fixture 1 must hold 0.8.
        let mut b = BTreeMap::new();
        b.insert(2, look(0.5));
        eng.go(&b, 0.0, FadeType::Linear);
        let state = eng.evaluate(Instant::now());
        assert_eq!(state.get(&1).unwrap().dimmer, 0.8);
        assert_eq!(state.get(&2).unwrap().dimmer, 0.5);
    }

    #[test]
    fn stop_fade_holds_midpoint() {
        let mut eng = LightingEngine::default();
        let mut a = BTreeMap::new();
        a.insert(1, look(1.0));
        eng.go(&a, 0.0, FadeType::Linear);
        let mut b = BTreeMap::new();
        b.insert(1, look(0.0));
        eng.go(&b, 100.0, FadeType::Linear); // glacial fade
        eng.stop_fade();
        let held = eng.evaluate(Instant::now()).get(&1).unwrap().dimmer;
        assert!(held > 0.95, "held near start value, got {held}");
        assert!(eng.fade.is_none());
    }
}
