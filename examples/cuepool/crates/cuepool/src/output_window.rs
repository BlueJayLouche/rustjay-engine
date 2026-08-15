use crate::video_pipeline::output_render_thread;
use crate::{App, MONITOR_MATCH_DIST_SQ, gpu_display_context};
use cuepool_core::LockExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use winit::event_loop::ActiveEventLoop;
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
    pub(crate) configured_index: usize,
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
    built
        .outputs
        .iter()
        .zip(live_outputs)
        .any(|(b, l)| b.monitor_id != l.monitor_id || b.fullscreen_monitor != l.fullscreen_monitor)
}

fn record_output_failure(
    failures: &mut Vec<String>,
    output_name: &str,
    stage: &str,
    error: impl std::fmt::Display,
    context: &str,
) {
    log::error!("Output '{output_name}' disabled at {stage}: {error}; {context}");
    failures.push(output_name.to_owned());
}

fn output_failure_summary(total: usize, active: usize, failures: &[String]) -> String {
    let failed = failures.join(", ");
    if active == 0 {
        format!(
            "No projection outputs could be opened ({failed}); cues continue without picture. See Window > Log."
        )
    } else {
        let noun = if failures.len() == 1 {
            "output"
        } else {
            "outputs"
        };
        format!(
            "Projection {noun} {failed} failed; {active} of {total} outputs remain active. See Window > Log."
        )
    }
}

fn surface_size_is_valid(width: u32, height: u32) -> bool {
    width > 0 && height > 0
}

fn output_runtime_role(configured_index: usize, active_outputs: usize) -> (usize, bool) {
    (configured_index, active_outputs == 0)
}

impl App {
    /// Toggle fullscreen on all output windows and update cursor visibility.
    pub(crate) fn toggle_output_fullscreen(&self) {
        for out in &self.output_windows {
            let currently_fullscreen = out.window.fullscreen().is_some();
            if currently_fullscreen {
                out.window.set_fullscreen(None);
                out.window.set_cursor_visible(true);
            } else {
                out.window
                    .set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                out.window.set_cursor_visible(false);
            }
        }
    }

    /// Create (or recreate) the video output window (starts windowed).
    /// Create (or recreate) one output window per configured projector output.
    pub(crate) fn create_output_windows(&mut self, event_loop: &ActiveEventLoop) {
        let projection = {
            let state = self.cuepool.state().lock_unpoisoned();
            state.show_file.projection.clone()
        };

        // Close existing output windows so we can honour new output counts/sizes.
        self.output_windows.clear();
        if let Some(ids) = self.window_ids.as_mut() {
            ids.video.clear();
        }

        // If nothing is configured yet, fall back to a single 1920x1080 output so
        // video playback still produces a window.
        let outputs: Vec<_> = if projection.outputs.is_empty() {
            vec![cuepool_core::ProjectorOutput::default_single()]
        } else {
            projection.outputs.clone()
        };

        // Snapshot what these windows are being built from (fallback applied), for
        // the structural-divergence check in about_to_wait.
        self.output_windows_built_from = Some(cuepool_core::ProjectionConfig {
            outputs: outputs.clone(),
            ..projection
        });

        let monitors: Vec<_> = event_loop.available_monitors().collect();

        // Resolve each output to a physical monitor by saved position descriptor
        // (survives reboots / projector warm-up reorder), falling back to the legacy
        // index for old projects. `assigned[i]` = Some(monitor index) or None (windowed).
        let mon_descs: Vec<cuepool_core::MonitorId> =
            monitors.iter().map(monitor_descriptor).collect();
        let wanted: Vec<Option<cuepool_core::MonitorId>> =
            outputs.iter().map(|o| o.monitor_id.clone()).collect();
        let mut assigned =
            cuepool_core::resolve_monitor_assignment(&wanted, &mon_descs, MONITOR_MATCH_DIST_SQ);
        let mut used = vec![false; monitors.len()];
        for a in assigned.iter().flatten() {
            used[*a] = true;
        }
        for (o, a) in outputs.iter().zip(assigned.iter_mut()) {
            if a.is_none()
                && let Some(idx) = o.fullscreen_monitor
                && idx < monitors.len()
                && !used[idx]
            {
                used[idx] = true;
                *a = Some(idx);
            }
        }

        // Windowed (un-assigned) outputs are tiled side-by-side at a preview size
        // that fits across the screen; assigned outputs go borderless-fullscreen.
        let windowed_count = assigned.iter().filter(|a| a.is_none()).count().max(1);
        let (screen_w, screen_h) = event_loop
            .primary_monitor()
            .or_else(|| monitors.first().cloned())
            .map(|m| {
                let sf = m.scale_factor();
                let s = m.size();
                (s.width as f64 / sf, s.height as f64 / sf)
            })
            .unwrap_or((1440.0, 900.0));
        let gap = 12.0;
        let tile_w = (((screen_w * 0.96) - gap * (windowed_count as f64 + 1.0))
            / windowed_count as f64)
            .max(160.0);
        let mut windowed_idx = 0usize;
        let mut pending_outputs = Vec::with_capacity(outputs.len());
        let mut failed_outputs = Vec::new();

        for (out_idx, output) in outputs.iter().enumerate() {
            let assigned_monitor = assigned[out_idx].and_then(|idx| mon_descs.get(idx));
            let context = gpu_display_context(&self.adapter, assigned_monitor);
            let mut attrs = winit::window::WindowAttributes::default()
                .with_title(format!("CuePool Output {}", output.name))
                .with_visible(true);

            if let Some(mon_idx) = assigned[out_idx] {
                if let Some(monitor) = monitors.get(mon_idx) {
                    attrs = attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(
                        Some(monitor.clone()),
                    )));
                }
            } else {
                let aspect = output.output_height.max(1) as f64 / output.output_width.max(1) as f64;
                let h = (tile_w * aspect).min(screen_h * 0.7);
                let x = gap + windowed_idx as f64 * (tile_w + gap);
                attrs = attrs
                    .with_inner_size(winit::dpi::LogicalSize::new(tile_w, h))
                    .with_position(winit::dpi::LogicalPosition::new(x, 80.0));
                windowed_idx += 1;
            }

            let window = match event_loop.create_window(attrs) {
                Ok(window) => Arc::new(window),
                Err(error) => {
                    record_output_failure(
                        &mut failed_outputs,
                        &output.name,
                        "window creation",
                        error,
                        &context,
                    );
                    continue;
                }
            };

            let surface = match self.instance.create_surface(Arc::clone(&window)) {
                Ok(surface) => surface,
                Err(error) => {
                    record_output_failure(
                        &mut failed_outputs,
                        &output.name,
                        "surface creation",
                        error,
                        &context,
                    );
                    continue;
                }
            };

            let size = window.inner_size();
            if !surface_size_is_valid(size.width, size.height) {
                record_output_failure(
                    &mut failed_outputs,
                    &output.name,
                    "surface configuration",
                    format_args!("window size is {}x{}", size.width, size.height),
                    &context,
                );
                continue;
            }
            let Some(mut config) =
                surface.get_default_config(&self.adapter, size.width, size.height)
            else {
                record_output_failure(
                    &mut failed_outputs,
                    &output.name,
                    "surface configuration",
                    format_args!(
                        "no supported configuration for {}x{}",
                        size.width, size.height
                    ),
                    &context,
                );
                continue;
            };
            let caps = surface.get_capabilities(&self.adapter);
            // The edge-blend brightness ramp is a linear-light multiply; it's only
            // correct if the GPU re-encodes to sRGB on write. Windows backends
            // default to a non-sRGB surface, so the blend band crushes to black.
            // Force an sRGB surface format when the surface offers one.
            if let Some(srgb) = caps.formats.iter().copied().find(|f| f.is_srgb()) {
                config.format = srgb;
            }
            // Present mode: EVERY output blocks on its own display's vsync
            // (Fifo). Each output renders on its own thread now, so a blocking
            // acquire paces only that thread — ungenlocked projectors no longer
            // serialize their vsync waits against each other. Override with
            // QPLAYER_PRESENT_MODE=fifo|fifo_relaxed|mailbox|immediate to force
            // one mode on every output.
            let want = match std::env::var("QPLAYER_PRESENT_MODE").as_deref() {
                Ok("mailbox") => wgpu::PresentMode::Mailbox,
                Ok("immediate") => wgpu::PresentMode::Immediate,
                Ok("fifo_relaxed") => wgpu::PresentMode::FifoRelaxed,
                _ => wgpu::PresentMode::Fifo,
            };
            config.present_mode = if caps.present_modes.contains(&want) {
                want
            } else {
                wgpu::PresentMode::Fifo
            };
            if matches!(
                config.present_mode,
                wgpu::PresentMode::Mailbox | wgpu::PresentMode::Immediate
            ) {
                log::warn!(
                    "Output '{}' uses {:?}, which free-runs; throttling to ~120 fps for safety",
                    output.name,
                    config.present_mode
                );
            }
            {
                let _configure_guard = self
                    .configure_gate
                    .write()
                    .unwrap_or_else(|e| e.into_inner());
                surface.configure(&self.device, &config);
            }

            if std::env::var("QPLAYER_FPS_DEBUG").is_ok() {
                let refresh = window
                    .current_monitor()
                    .and_then(|m| m.refresh_rate_millihertz())
                    .map(|mhz| format!("{:.2} Hz", mhz as f64 / 1000.0))
                    .unwrap_or_else(|| "?".into());
                eprintln!(
                    "OUTPUT '{}': present_mode={:?} format={:?} refresh={} fullscreen={}",
                    output.name,
                    config.present_mode,
                    config.format,
                    refresh,
                    window.fullscreen().is_some(),
                );
            }

            let pixel_perfect = output.output_width == output.source_width
                && output.output_height == output.source_height;
            let renderer =
                cuepool_video::ProjectionRenderer::new(&self.device, config.format, pixel_perfect);

            let size_atomic = Arc::new(AtomicU64::new(pack_size(size.width, size.height)));
            let stop = Arc::new(AtomicBool::new(false));
            let presented = Arc::new(AtomicU32::new(0));
            let present_mode = config.present_mode;
            let format = config.format;
            pending_outputs.push((
                out_idx,
                output.clone(),
                window,
                surface,
                config,
                renderer,
                size_atomic,
                stop,
                presented,
                present_mode,
                format,
                context,
            ));
        }

        // All surfaces must be configured before any render thread can submit.
        for (
            out_idx,
            output_config,
            window,
            surface,
            config,
            renderer,
            size_atomic,
            stop,
            presented,
            present_mode,
            format,
            context,
        ) in pending_outputs
        {
            let (configured_index, paces_video) =
                output_runtime_role(out_idx, self.output_windows.len());
            let video_id = window.id();
            let frame_state = Arc::clone(&self.frame_state);
            let vsync_tick = Arc::clone(&self.vsync_tick);
            let configure_gate = Arc::clone(&self.configure_gate);
            let thread_size = Arc::clone(&size_atomic);
            let thread_stop = Arc::clone(&stop);
            let thread_presented = Arc::clone(&presented);
            let device = self.device.clone();
            let queue = self.queue.clone();
            let event_loop_proxy = self.event_loop_proxy.clone();
            let fallback_output = output_config.clone();
            let join = match std::thread::Builder::new()
                .name(format!("output-render-{}", output_config.name))
                .spawn(move || {
                    output_render_thread(
                        surface,
                        config,
                        renderer,
                        device,
                        queue,
                        configure_gate,
                        event_loop_proxy,
                        frame_state,
                        vsync_tick,
                        thread_size,
                        thread_stop,
                        thread_presented,
                        video_id,
                        configured_index,
                        paces_video,
                        fallback_output,
                    );
                }) {
                Ok(join) => join,
                Err(e) => {
                    // Skipping the pushes below drops the window and registers
                    // nothing, so the failed output simply stays dark while the
                    // rest of the show carries on.
                    record_output_failure(
                        &mut failed_outputs,
                        &output_config.name,
                        "render-thread spawn",
                        e,
                        &context,
                    );
                    continue;
                }
            };
            self.output_windows.push(OutputWindow {
                id: video_id,
                window,
                configured_index,
                output_config,
                size: size_atomic,
                stop,
                presented,
                present_mode,
                format,
                join: Some(join),
            });

            if let Some(ids) = self.window_ids.as_mut() {
                ids.video.push(video_id);
            }
        }

        if !failed_outputs.is_empty() {
            let message =
                output_failure_summary(outputs.len(), self.output_windows.len(), &failed_outputs);
            self.cuepool
                .state()
                .lock_unpoisoned()
                .report_operator_error(message);
        }

        // Freshly created (fullscreen) output windows grab the foreground — on
        // Windows they bury the control window, and auto-rebuilds make that a
        // surprise. Pull the GUI back to the front.
        if let Some(control) = &self.control_window {
            control.focus_window();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{output_failure_summary, output_runtime_role, surface_size_is_valid};

    #[test]
    fn output_failure_summary_distinguishes_partial_and_total_failure() {
        assert_eq!(
            output_failure_summary(3, 2, &["Projector 2".into()]),
            "Projection output Projector 2 failed; 2 of 3 outputs remain active. See Window > Log."
        );
        assert_eq!(
            output_failure_summary(2, 0, &["Left".into(), "Right".into()]),
            "No projection outputs could be opened (Left, Right); cues continue without picture. See Window > Log."
        );
    }

    #[test]
    fn output_failures_reject_zero_surfaces() {
        assert!(!surface_size_is_valid(1920, 0));
        assert!(!surface_size_is_valid(0, 1080));
    }

    #[test]
    fn partial_output_failure_preserves_runtime_roles() {
        assert_eq!(output_runtime_role(1, 0), (1, true));
        assert_eq!(output_runtime_role(2, 1), (2, false));
    }
}
