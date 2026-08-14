//! DMX recorder — wire capture → per-channel punch-in pass → `.dmxrec` takes.
//!
//! While a pass runs, a [`DmxReceiver`] listens for sACN + Art-Net on the
//! standard ports (unicast/broadcast; sACN multicast universes are not joined
//! yet) and feeds a [`PunchRecorder`]. The merged monitor state is pushed
//! into the [`LightingEngine`] as an overlay layer each tick, so the rig
//! plays base take + live punches through the normal output path.
//!
//! Take safety: the raw pass streams to `<file>.pass` while recording; on
//! Stop & Keep the previous take moves to `<file>.prev` (single-level revert)
//! before the merged take is written.

use std::path::{Path, PathBuf};
use std::time::Instant;

use cuepool_gui::app::RecorderStatus;
use rustjay_lighting::{DmxReceiver, MaskedFrame, PunchRecorder, RecWriter, RxConfig, read_rec};

use crate::lighting_engine::LightingEngine;

/// Merge priority of the monitor overlay — live input outranks playback and
/// looks by default (design Q6). Made configurable with the OSC bridge phase.
const MONITOR_PRIORITY: u8 = 150;

struct Pass {
    punch: PunchRecorder,
    started: Instant,
    file: PathBuf,
    rx: DmxReceiver,
}

pub struct Recorder {
    pass: Option<Pass>,
    pub monitor: bool,
    /// Live bridge state: latest OSC/MIDI channel values (latest wins,
    /// held until cleared). Output as the overlay layer while idle; fed
    /// into the punch recorder while a pass runs.
    live: MaskedFrame,
    /// Take-editor scrub frame — held on the output, wins over the bridge.
    scrub: Option<MaskedFrame>,
    /// Socket setup for the pass receiver (standard ports by default;
    /// tests override to ephemeral so other sACN apps can't eat packets).
    rx_config: RxConfig,
    rx_packets: u64,
    error: Option<String>,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            pass: None,
            monitor: true,
            live: MaskedFrame::default(),
            scrub: None,
            rx_config: RxConfig::default(),
            rx_packets: 0,
            error: None,
        }
    }

    pub fn recording(&self) -> bool {
        self.pass.is_some()
    }

    /// Live OSC/MIDI channel input (`channel` 0-based). Always lands in the
    /// bridge state; during a pass it also feeds punch-in like wire input.
    pub fn live_input(&mut self, universe: u16, channel: u16, value: u8) {
        self.live.set(universe, channel, value);
        if let Some(pass) = &mut self.pass {
            let t_ms = pass.started.elapsed().as_millis() as u32;
            pass.punch.input(t_ms, universe, channel, value);
        }
    }

    /// Release every channel the live bridge holds.
    pub fn clear_live(&mut self) {
        self.live = MaskedFrame::default();
    }

    /// Hold a take-editor scrub frame on the output (None releases it).
    pub fn set_scrub(&mut self, frame: Option<MaskedFrame>) {
        self.scrub = frame;
    }

    /// Start a pass on `file`, or stop-and-keep the running one.
    pub fn record_toggle(&mut self, file: &str) {
        if self.pass.is_some() {
            self.stop_keep();
        } else {
            self.error = None;
            if let Err(e) = self.start(file) {
                log::error!("Recorder: {e}");
                self.error = Some(e);
            }
        }
    }

    fn start(&mut self, file: &str) -> Result<(), String> {
        let file = PathBuf::from(file.trim());
        if file.as_os_str().is_empty() {
            return Err("no recording file set".into());
        }
        // Existing take = punch-in base; missing file = fresh recording.
        let base = if file.exists() {
            read_rec(&file).map_err(|e| format!("cannot read take '{}': {e}", file.display()))?
        } else {
            Vec::new()
        };
        let pass_path = pass_path(&file);
        let punch = PunchRecorder::start(base, Some(&pass_path))
            .map_err(|e| format!("cannot write '{}': {e}", pass_path.display()))?;
        let rx = DmxReceiver::spawn(self.rx_config.clone())
            .map_err(|e| format!("cannot open DMX input sockets: {e}"))?;
        log::info!("Recorder: pass started on '{}'", file.display());
        self.rx_packets = 0;
        self.pass = Some(Pass {
            punch,
            started: Instant::now(),
            file,
            rx,
        });
        Ok(())
    }

    fn stop_keep(&mut self) {
        let Some(pass) = self.pass.take() else { return };
        let file = pass.file;
        pass.rx.shutdown();
        let result = pass.punch.finish().and_then(|take| {
            // Single-level revert: current take becomes .prev.
            if file.exists() {
                std::fs::rename(&file, prev_path(&file))?;
            }
            let mut w = RecWriter::create(&file)?;
            for e in &take {
                w.write(*e)?;
            }
            w.finish()?;
            std::fs::remove_file(pass_path(&file)).ok();
            Ok(take.len())
        });
        match result {
            Ok(n) => log::info!("Recorder: take kept — {n} event(s) in '{}'", file.display()),
            Err(e) => {
                // The streamed .pass file is left in place for manual rescue.
                log::error!("Recorder: keeping take failed: {e}");
                self.error = Some(format!("keeping take failed: {e}"));
            }
        }
    }

    /// Stop and throw away the in-flight pass; the take on disk is untouched.
    pub fn discard(&mut self) {
        if let Some(pass) = self.pass.take() {
            pass.rx.shutdown();
            std::fs::remove_file(pass_path(&pass.file)).ok();
            log::info!("Recorder: pass discarded");
        }
    }

    /// Swap the take with its `.prev` (undo the last kept pass). Idle only.
    pub fn revert(&mut self, file: &str) {
        if self.pass.is_some() {
            return;
        }
        let file = PathBuf::from(file.trim());
        let prev = prev_path(&file);
        if !prev.exists() {
            return;
        }
        let tmp = file.with_extension("dmxrec.swap");
        let swapped = (|| -> std::io::Result<()> {
            if file.exists() {
                std::fs::rename(&file, &tmp)?;
            }
            std::fs::rename(&prev, &file)?;
            if tmp.exists() {
                std::fs::rename(&tmp, &prev)?;
            }
            Ok(())
        })();
        match swapped {
            Ok(()) => log::info!("Recorder: reverted '{}' to previous take", file.display()),
            Err(e) => {
                log::error!("Recorder: revert failed: {e}");
                self.error = Some(format!("revert failed: {e}"));
            }
        }
    }

    /// Per-frame: drain received DMX into the pass and refresh the monitor
    /// overlay. While idle the overlay carries the live bridge instead.
    pub fn tick(&mut self, lighting: &mut LightingEngine) {
        let Some(pass) = &mut self.pass else {
            // Editor scrub wins over the live bridge while it's held.
            let overlay = self
                .scrub
                .clone()
                .or_else(|| (!self.live.is_empty()).then(|| self.live.clone()));
            lighting.set_overlay(overlay.map(|f| (MONITOR_PRIORITY, f)));
            return;
        };
        let t_ms = pass.started.elapsed().as_millis() as u32;
        while let Ok(pkt) = pass.rx.packets().try_recv() {
            self.rx_packets += 1;
            pass.punch.input_universe(t_ms, pkt.universe, &pkt.data);
        }
        lighting.set_overlay(
            self.monitor
                .then(|| (MONITOR_PRIORITY, pass.punch.monitor_frame(t_ms))),
        );
    }

    pub fn snapshot(&self) -> RecorderStatus {
        RecorderStatus {
            recording: self.pass.is_some(),
            elapsed_s: self
                .pass
                .as_ref()
                .map_or(0.0, |p| p.started.elapsed().as_secs_f32()),
            event_count: self.pass.as_ref().map_or(0, |p| p.punch.event_count()),
            punched_count: self.pass.as_ref().map_or(0, |p| p.punch.punched_count()),
            rx_packets: self.rx_packets,
            error: self.error.clone(),
        }
    }
}

fn pass_path(file: &Path) -> PathBuf {
    let mut p = file.as_os_str().to_owned();
    p.push(".pass");
    PathBuf::from(p)
}

fn prev_path(file: &Path) -> PathBuf {
    let mut p = file.as_os_str().to_owned();
    p.push(".prev");
    PathBuf::from(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustjay_lighting::{Dest, DmxFrame, DmxTransport, SacnTransport};
    use std::net::Ipv4Addr;
    use std::time::Duration;

    /// Fresh take end-to-end through the real sockets: record_toggle → sACN
    /// unicast to the standard port → tick drains → stop & keep → file.
    #[test]
    fn record_pass_end_to_end() {
        let file = std::env::temp_dir().join(format!(
            "cuepool-recorder-e2e-{}.dmxrec",
            std::process::id()
        ));
        std::fs::remove_file(&file).ok();
        let file_s = file.to_string_lossy().to_string();

        let mut lighting = LightingEngine::default();
        let mut rec = Recorder::new();
        // Ephemeral port: a real sACN app on :5568 (reuseport) would steal
        // the unicast test packets.
        rec.rx_config = RxConfig {
            artnet: false,
            sacn_port: 0,
            ..Default::default()
        };
        rec.record_toggle(&file_s);
        let snap = rec.snapshot();
        assert!(snap.recording, "pass should start: {:?}", snap.error);
        let sacn_port = rec.pass.as_ref().unwrap().rx.ports().0;

        let mut tx = SacnTransport::new(Dest::Unicast(Ipv4Addr::LOCALHOST), 100, "rec-e2e")
            .unwrap()
            .with_dest_port(sacn_port);
        for v in [10u8, 20] {
            let mut frame = DmxFrame::new();
            frame.universe_mut(1)[0] = v;
            tx.send(&frame);
            std::thread::sleep(Duration::from_millis(60));
            rec.tick(&mut lighting);
        }
        assert!(rec.snapshot().event_count >= 2, "both changes captured");

        rec.record_toggle(&file_s); // stop & keep
        assert!(!rec.snapshot().recording);
        let take = read_rec(&file).unwrap();
        let values: Vec<u8> = take
            .iter()
            .filter(|e| e.universe == 1 && e.channel == 0)
            .map(|e| e.value)
            .collect();
        assert_eq!(values, vec![10, 20]);
        assert!(!pass_path(&file).exists(), ".pass cleaned up on keep");
        std::fs::remove_file(&file).ok();
    }

    /// Live bridge: idle input becomes the overlay layer; clear releases it.
    #[test]
    fn live_bridge_idle_overlay() {
        let mut lighting = LightingEngine::default();
        let mut rec = Recorder::new();

        rec.tick(&mut lighting); // idle + empty bridge → no overlay
        rec.live_input(1, 0, 200);
        rec.tick(&mut lighting);
        // The overlay is engine-internal; verify via the recorder's state.
        assert!(!rec.live.is_empty(), "bridge holds the value");
        assert_eq!(rec.live.get(1).unwrap().values()[0], 200);

        rec.clear_live();
        assert!(rec.live.is_empty());
        rec.tick(&mut lighting); // must not panic with empty bridge
    }

    /// Live input during a pass punches in like wire input.
    #[test]
    fn live_input_feeds_recording_pass() {
        let file = std::env::temp_dir().join(format!(
            "cuepool-recorder-osc-{}.dmxrec",
            std::process::id()
        ));
        std::fs::remove_file(&file).ok();
        let file_s = file.to_string_lossy().to_string();

        let mut rec = Recorder::new();
        rec.rx_config = RxConfig {
            artnet: false,
            sacn_port: 0,
            ..Default::default()
        };
        rec.record_toggle(&file_s);
        assert!(rec.recording());

        rec.live_input(2, 5, 99); // 0-based channel 5
        assert_eq!(rec.snapshot().event_count, 1);
        rec.record_toggle(&file_s); // stop & keep

        let take = read_rec(&file).unwrap();
        assert_eq!(
            (take[0].universe, take[0].channel, take[0].value),
            (2, 5, 99)
        );
        std::fs::remove_file(&file).ok();
    }
}
