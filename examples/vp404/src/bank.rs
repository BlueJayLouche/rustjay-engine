//! A bank of pads + the shared handle the egui grid tab talks to.
//!
//! Same split as `examples/shaderglass`: the render thread owns the live `Bank`
//! (pads decode on the GPU); the UI tab holds a [`BankHandle`] clone and
//! communicates by posting [`PadCmd`]s (drained in `prepare`) and reading a
//! published [`PadInfo`] roster. Sample loading needs a `wgpu::Device`, which is
//! only available render-side — hence the command queue.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::pad::{Pad, TriggerMode};

/// Default pad count. ponytail: 16 for now — full count-as-setting is later polish.
pub const PAD_COUNT: usize = 16;

pub struct Bank {
    pub pads: Vec<Pad>,
    /// Last pad triggered — used by the UI to highlight the most recent pad.
    #[allow(dead_code)]
    pub last_triggered: Option<usize>,
}

impl Bank {
    pub fn new(n: usize) -> Self {
        Self {
            pads: (0..n).map(Pad::new).collect(),
            last_triggered: None,
        }
    }

    /// The pad to display: last-triggered if still playing, else any playing pad.
    #[allow(dead_code)]
    pub fn active(&self) -> Option<usize> {
        self.last_triggered
            .filter(|&i| self.pads.get(i).is_some_and(|p| p.is_playing))
            .or_else(|| self.pads.iter().position(|p| p.is_playing))
    }
}

/// A UI → render-thread command.
#[derive(Clone)]
pub enum PadCmd {
    Trigger(usize),
    Release(usize),
    Load(usize, PathBuf),
    Clear(usize),
    SetMode(usize, TriggerMode),
    SetRange(usize, u32, u32),
    /// Start live-sampling `frame_count` frames into the given pad.
    #[cfg(feature = "capture")]
    StartSampling(usize, u32),
    /// Cancel the current live-sampling session.
    #[cfg(feature = "capture")]
    StopSampling,
}

/// Per-pad display state published render → UI each frame (roster index = pad index).
#[derive(Clone, Default)]
pub struct PadInfo {
    pub name: String,
    pub color: [u8; 3],
    pub loaded: bool,
    pub playing: bool,
    pub progress: f32,
    pub trigger_mode: TriggerMode,
    pub beat_division: usize,
    pub in_point: u32,
    pub out_point: u32,
    pub frame_count: u32,
}

/// Live-sampler status published render → UI each frame.
#[cfg(feature = "capture")]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SamplerStatus {
    #[default]
    Idle,
    Recording,
    Encoding,
    Error,
}

/// Shared handle: the tab clones this; plugin and tab share the same Arcs.
#[derive(Clone)]
pub struct BankHandle {
    pub cmds: Arc<Mutex<Vec<PadCmd>>>,
    pub roster: Arc<Mutex<Vec<PadInfo>>>,
    #[cfg(feature = "capture")]
    pub sampler_status: Arc<Mutex<SamplerStatus>>,
}

impl BankHandle {
    pub fn new() -> Self {
        Self {
            cmds: Arc::new(Mutex::new(Vec::new())),
            roster: Arc::new(Mutex::new(Vec::new())),
            #[cfg(feature = "capture")]
            sampler_status: Arc::new(Mutex::new(SamplerStatus::Idle)),
        }
    }

    pub fn post(&self, cmd: PadCmd) {
        if let Ok(mut g) = self.cmds.lock() {
            g.push(cmd);
        }
    }

    pub fn roster(&self) -> Vec<PadInfo> {
        self.roster.lock().map(|r| r.clone()).unwrap_or_default()
    }

    #[cfg(feature = "capture")]
    pub fn sampler_status(&self) -> SamplerStatus {
        self.sampler_status.lock().map(|s| *s).unwrap_or_default()
    }

    #[cfg(feature = "capture")]
    pub fn set_sampler_status(&self, status: SamplerStatus) {
        if let Ok(mut g) = self.sampler_status.lock() {
            *g = status;
        }
    }
}

impl Default for BankHandle {
    fn default() -> Self {
        Self::new()
    }
}
