use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use winit::window::{Window, WindowId};

/// Build a stable-ish descriptor from a winit monitor (name + resolution + position).
pub(crate) fn monitor_descriptor(m: &winit::monitor::MonitorHandle) -> cuepool_core::MonitorId {
    let pos = m.position();
    let size = m.size();
    cuepool_core::MonitorId {
        name: m.name().unwrap_or_default(),
        width: size.width,
        height: size.height,
        pos_x: pos.x,
        pos_y: pos.y,
    }
}

/// Per-window identifiers so we can route events.
pub(crate) struct WindowIds {
    pub(crate) control: WindowId,
    pub(crate) video: Vec<WindowId>,
}

/// One projector output window. The main (winit) thread owns the window itself
/// (events, fullscreen toggles); the surface, its config and the slice renderer
/// live on a dedicated render thread that blocks on THIS display's vsync
/// (Fifo), so ungenlocked outputs never serialize against each other.
pub(crate) struct OutputWindow {
    pub(crate) id: WindowId,
    pub(crate) window: Arc<Window>,
    /// Baked snapshot: display name (identify/diagnostics) and the fallback
    /// used when the live projection outputs list has no entry for this window.
    pub(crate) output_config: cuepool_core::ProjectorOutput,
    /// Latest window size forwarded to the render thread (packed `w<<32 | h`).
    pub(crate) size: Arc<AtomicU64>,
    /// Stop signal for the render thread.
    pub(crate) stop: Arc<AtomicBool>,
    /// Presents completed by the render thread (drained ~1 Hz for diagnostics).
    pub(crate) presented: Arc<AtomicU32>,
    pub(crate) present_mode: wgpu::PresentMode,
    pub(crate) format: wgpu::TextureFormat,
    pub(crate) join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for OutputWindow {
    /// Signal the render thread, but never let a wedged driver call freeze winit.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let deadline = Instant::now() + Duration::from_millis(250);
            while !join.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if join.is_finished() {
                let _ = join.join();
            } else {
                // Detaching is the lesser evil: the worker owns its Surface and
                // cloned GPU/state handles. `create_surface(Arc<Window>)` keeps
                // that window alive until the worker eventually drops the Surface.
                log::error!(
                    "Output '{}' render thread did not stop within 250 ms; detaching",
                    self.output_config.name,
                );
            }
        }
    }
}

pub(crate) fn pack_size(w: u32, h: u32) -> u64 {
    ((w as u64) << 32) | h as u64
}

pub(crate) fn unpack_size(packed: u64) -> (u32, u32) {
    ((packed >> 32) as u32, packed as u32)
}

/// True when the parts of the projection that window creation bakes in differ:
/// output count and monitor assignment. Everything else travels per-frame now:
/// source rect and edge blend ride the live uniforms, and the canvas texture is
/// resized on playback start, so geometry/canvas edits must NOT rebuild windows
/// (a DragValue edit would otherwise storm window recreation and bury the GUI).
/// ponytail: `pixel_perfect` (the sampler filter baked at renderer creation)
/// goes stale when a geometry edit flips whether output size == source size —
/// a filtering nit, not a correctness bug; upgrade path is a manual rebuild via
/// "Open Projection Output Windows".
pub(crate) fn projection_structure_changed(
    built: &cuepool_core::ProjectionConfig,
    live: &cuepool_core::ProjectionConfig,
) -> bool {
    // Same fallback as create_output_windows: no configured outputs = one default.
    let default;
    let live_outputs: &[cuepool_core::ProjectorOutput] = if live.outputs.is_empty() {
        default = cuepool_core::ProjectorOutput::default_single();
        std::slice::from_ref(&default)
    } else {
        live.outputs.as_slice()
    };
    if built.outputs.len() != live_outputs.len() {
        return true;
    }
    built.outputs.iter().zip(live_outputs).any(|(b, l)| {
        b.monitor_id != l.monitor_id || b.fullscreen_monitor != l.fullscreen_monitor
    })
}
